use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_federation::access;
use gitquarry_federation::entities::{Peer, PeerAccessLevel};
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use gitquarry_test_support::{federation_state, send};
use gitquarry_wire::Uid;
use std::sync::Arc;

/// The comment form as core proxies it, with the session uid core forwards
/// when there is one.
async fn post_comment(state: &Arc<FederationState>, uid: Option<&Uid>) -> StatusCode {
    let mut req = Request::post("/_remote_pulls/comment").header("content-type", "application/json");
    if let Some(uid) = uid {
        req = req.header("x-gitquarry-user-uid", uid.to_string());
    }
    let body = serde_json::json!({"peer": "acme", "repo": "acme/demo", "number": 1, "body": "hi"});
    let req = req.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    send(&router(Arc::clone(state)), req).await.status
}

#[tokio::test]
async fn rejects_without_the_forwarded_uid_header() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    assert_eq!(post_comment(&state, None).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_a_peer_thats_not_full_access_level() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        linked_at: String::new(),
        access_level: PeerAccessLevel::BrowseAndFork,
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();

    let uid = Uid::new();
    assert_eq!(post_comment(&state, Some(&uid)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rejects_without_a_linked_credential() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        linked_at: String::new(),
        access_level: PeerAccessLevel::Full,
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();

    let uid = Uid::new();
    assert_eq!(post_comment(&state, Some(&uid)).await, StatusCode::FORBIDDEN);
}
