use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_federation::access;
use gitquarry_federation::entities::{Peer, PeerAccessLevel};
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use gitquarry_wire::Uid;
use s3db::LocalBackend;
use std::sync::Arc;
use tower::ServiceExt;

fn test_state(root: &std::path::Path) -> Arc<FederationState> {
    let backend = LocalBackend::new(&s3db::LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: s3db::LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[33u8; s3db_crypto::SALT_LEN]).unwrap();
    std::sync::Arc::new(gitquarry_federation::state::FederationState {
        backend: std::sync::Arc::new(backend),
        cipher: std::sync::Arc::new(cipher),
        core: gitquarry_federation::core_client::CoreClient::new(
            "http://127.0.0.1:1".to_string(),
            "unused".to_string(),
        ),
        backend_config: s3db::BackendConfig::Local(s3db::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: s3db::LocalEngine::Files,
        }),
        gossip: std::sync::Arc::new(gitquarry_federation::gossip::GossipTable::default()),
        self_announce: None,
        rate_limiter: std::sync::Arc::new(gitquarry_federation::rate_limit::PeerRateLimiter::default()),
        rate_limit_public_max: 60,
        rate_limit_public_window: std::time::Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: std::time::Duration::from_secs(60),
        rate_limit_peer_override_by_ip: true,
    })
}

/// The comment form as core proxies it, with the session uid core forwards
/// when there is one.
async fn post_comment(state: &Arc<FederationState>, uid: Option<&Uid>) -> StatusCode {
    let mut req = Request::post("/_remote_pulls/comment").header("content-type", "application/json");
    if let Some(uid) = uid {
        req = req.header("x-gitquarry-user-uid", uid.to_string());
    }
    let body = serde_json::json!({"peer": "acme", "repo": "acme/demo", "number": 1, "body": "hi"});
    let req = req.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    router(Arc::clone(state)).oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn rejects_without_the_forwarded_uid_header() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    assert_eq!(post_comment(&state, None).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_a_peer_thats_not_full_access_level() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
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
    let state = test_state(dir.path());
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
