use gitquarry_federation::entities::Peer;
use gitquarry_federation::sister::SisterBanEntry;
use gitquarry_federation::web::router;
use gitquarry_federation::{access, sister};
use gitquarry_test_support::{Page, federation_state, send};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};

fn handshake(body: &[u8]) -> Request<Body> {
    Request::post("/_federation/sister/handshake")
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn with_bearer(req: axum::http::request::Builder, secret: Option<&str>) -> axum::http::request::Builder {
    match secret {
        Some(secret) => req.header(header::AUTHORIZATION, format!("Bearer {secret}")),
        None => req,
    }
}

fn ban_event(secret: Option<&str>, body: Vec<u8>) -> Request<Body> {
    with_bearer(Request::post("/_federation/sister/ban-event"), secret)
        .body(Body::from(body))
        .unwrap()
}

fn bans(secret: Option<&str>) -> Request<Body> {
    with_bearer(Request::get("/_federation/sister/bans"), secret)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn handshake_confirms_only_once_this_side_already_requested() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();

    let payload = serde_json::to_vec(&serde_json::json!({ "instance_name": "acme", "secret": "shh" })).unwrap();
    let body = send(&router(Arc::clone(&state)), handshake(&payload)).await.body;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["ok"], false);

    sister::request(state.backend.as_ref(), &state.cipher, "acme")
        .await
        .unwrap();
    let body = send(&router(Arc::clone(&state)), handshake(&payload)).await.body;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["ok"], true);
}

#[tokio::test]
async fn receive_handshake_rejects_a_different_secret_once_already_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();
    sister::request(state.backend.as_ref(), &state.cipher, "acme")
        .await
        .unwrap();

    let confirmed = sister::receive_handshake(state.backend.as_ref(), &state.cipher, "acme", "real-secret")
        .await
        .unwrap();
    assert!(confirmed);

    // An unauthenticated caller who knows "acme" was requested as a sister
    // can't hijack the relationship by re-POSTing a different secret once
    // it's already confirmed.
    let hijacked = sister::receive_handshake(state.backend.as_ref(), &state.cipher, "acme", "attacker-secret")
        .await
        .unwrap();
    assert!(!hijacked);
    assert_eq!(
        sister::credential_for(state.backend.as_ref(), &state.cipher, "acme")
            .await
            .unwrap()
            .as_deref(),
        Some("real-secret")
    );

    // Retrying with the *same* secret stays idempotent (a lost response
    // shouldn't strand the initiator in a permanent "waiting" state).
    let retried = sister::receive_handshake(state.backend.as_ref(), &state.cipher, "acme", "real-secret")
        .await
        .unwrap();
    assert!(retried);
}

#[tokio::test]
async fn ban_event_and_bans_get_require_a_valid_sister_secret() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    assert_eq!(
        send(&router(Arc::clone(&state)), ban_event(None, Vec::new()))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&router(Arc::clone(&state)), bans(None)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&router(Arc::clone(&state)), bans(Some("guessed"))).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn ban_event_applies_and_bans_get_reflects_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();
    sister::request(state.backend.as_ref(), &state.cipher, "acme")
        .await
        .unwrap();
    sister::receive_handshake(state.backend.as_ref(), &state.cipher, "acme", "shared-secret")
        .await
        .unwrap();

    let event = serde_json::to_vec(&serde_json::json!({
        "peer_name": "stranger", "git_base_url": "https://stranger.example",
        "action": "ban", "reason": "abuse", "banned_by": "bob", "banned_at": null
    }))
    .unwrap();
    assert_eq!(
        send(&router(Arc::clone(&state)), ban_event(Some("shared-secret"), event))
            .await
            .status,
        StatusCode::OK
    );

    let Page { status, body, .. } = send(&router(Arc::clone(&state)), bans(Some("shared-secret"))).await;
    assert_eq!(status, StatusCode::OK);
    let list: Vec<SisterBanEntry> = serde_json::from_str(&body).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].peer_name, "stranger");
}
