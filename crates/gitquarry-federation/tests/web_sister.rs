use gitquarry_federation::entities::Peer;
use gitquarry_federation::gossip::GossipCandidateWire;
use gitquarry_federation::sister::SisterBanEntry;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use gitquarry_federation::{access, instance_client, sister};
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

/// A federation instance on a real socket, announcing itself as `name` at
/// its own address — the dial-back needs both.
async fn spawn_instance(root: &std::path::Path, name: &str) -> (Arc<FederationState>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(FederationState {
        self_announce: Some(GossipCandidateWire {
            name: name.to_string(),
            web_base_url: url.clone(),
            git_base_url: url.clone(),
        }),
        ..FederationState::clone(&federation_state(root, "http://127.0.0.1:1"))
    });
    let app = router(Arc::clone(&state));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, url)
}

async fn link(state: &FederationState, name: &str, url: &str) {
    let peer = Peer {
        name: name.to_string(),
        web_base_url: url.to_string(),
        git_base_url: url.to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();
}

async fn confirmed(state: &FederationState, name: &str) -> bool {
    access::get_peer(state.backend.as_ref(), &state.cipher, name)
        .await
        .unwrap()
        .unwrap()
        .sister_confirmed
}

#[tokio::test]
async fn a_handshake_confirms_on_both_sides_once_each_side_requested() {
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, a_url) = spawn_instance(dir_a.path(), "instance-a").await;
    let (b, b_url) = spawn_instance(dir_b.path(), "instance-b").await;
    link(&a, "instance-b", &b_url).await;
    link(&b, "instance-a", &a_url).await;

    let secret_a = sister::request(a.backend.as_ref(), &a.cipher, "instance-b")
        .await
        .unwrap();
    assert!(
        !instance_client::sister_handshake(&b_url, "instance-a", &secret_a)
            .await
            .unwrap()
    );

    let secret_b = sister::request(b.backend.as_ref(), &b.cipher, "instance-a")
        .await
        .unwrap();
    assert!(
        instance_client::sister_handshake(&a_url, "instance-b", &secret_b)
            .await
            .unwrap()
    );
    sister::confirm_after_handshake(b.backend.as_ref(), &b.cipher, "instance-a", &secret_b)
        .await
        .unwrap();

    assert!(confirmed(&a, "instance-b").await && confirmed(&b, "instance-a").await);
    let held_by_a = sister::credential_for(a.backend.as_ref(), &a.cipher, "instance-b")
        .await
        .unwrap();
    assert_eq!(held_by_a.as_deref(), Some(secret_b.as_str()));
}

#[tokio::test]
async fn a_forged_handshake_is_refused_because_the_real_peer_wont_vouch_for_it() {
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, a_url) = spawn_instance(dir_a.path(), "instance-a").await;
    let (_b, b_url) = spawn_instance(dir_b.path(), "instance-b").await;
    link(&a, "instance-b", &b_url).await;
    sister::request(a.backend.as_ref(), &a.cipher, "instance-b")
        .await
        .unwrap();

    // Anyone can claim to be instance-b while a's request is pending; a
    // asks the real instance-b, which never staged this secret.
    assert!(
        !instance_client::sister_handshake(&a_url, "instance-b", "attacker-secret")
            .await
            .unwrap()
    );
    assert!(!confirmed(&a, "instance-b").await);
    assert!(
        sister::sister_by_secret(a.backend.as_ref(), &a.cipher, "attacker-secret")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn receive_handshake_rejects_a_different_secret_once_already_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    link(&state, "acme", "https://acme.example").await;
    sister::request(state.backend.as_ref(), &state.cipher, "acme")
        .await
        .unwrap();
    sister::confirm_after_handshake(state.backend.as_ref(), &state.cipher, "acme", "real-secret")
        .await
        .unwrap();

    // An unauthenticated caller who knows "acme" is a sister can't hijack
    // the relationship by re-POSTing a different secret once it's
    // confirmed.
    let hijacked = sister::receive_handshake(state.backend.as_ref(), &state.cipher, "me", "acme", "attacker-secret")
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
    let retried = sister::receive_handshake(state.backend.as_ref(), &state.cipher, "me", "acme", "real-secret")
        .await
        .unwrap();
    assert!(retried);
}

#[tokio::test]
async fn a_handshake_without_an_own_identity_cannot_be_verified() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let payload = serde_json::to_vec(&serde_json::json!({ "instance_name": "acme", "secret": "shh" })).unwrap();
    assert_eq!(
        send(&router(Arc::clone(&state)), handshake(&payload)).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
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
    sister::confirm_after_handshake(state.backend.as_ref(), &state.cipher, "acme", "shared-secret")
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

    let stranger = access::get_peer(state.backend.as_ref(), &state.cipher, "stranger")
        .await
        .unwrap()
        .unwrap();
    assert!(stranger.banned);

    let local = Peer {
        name: "spammer".to_string(),
        web_base_url: "https://spammer.example".to_string(),
        git_base_url: "https://spammer.example".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &local, None)
        .await
        .unwrap();
    gitquarry_federation::domain::peer::ban(state.backend.as_ref(), &state.cipher, "spammer", "spam", "admin")
        .await
        .unwrap();

    // "stranger" is acme's own ban, so acme never gets it handed back —
    // only what this instance decided itself.
    let Page { status, body, .. } = send(&router(Arc::clone(&state)), bans(Some("shared-secret"))).await;
    assert_eq!(status, StatusCode::OK);
    let list: Vec<SisterBanEntry> = serde_json::from_str(&body).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].peer_name, "spammer");
}
