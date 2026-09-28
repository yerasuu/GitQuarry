use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_federation::access;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::gossip::GossipTable;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use gitquarry_wire::InstanceInfo;
use s3db::{BackendConfig, LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;
use tower::ServiceExt;

fn test_state(root: &std::path::Path) -> Arc<FederationState> {
    let bucket = root.to_string_lossy().to_string();
    let backend = LocalBackend::new(&LocalParams {
        bucket: bucket.clone(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[44u8; s3db_crypto::SALT_LEN]).unwrap();
    Arc::new(FederationState {
        backend: Arc::new(backend),
        cipher: Arc::new(cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
        backend_config: BackendConfig::Local(LocalParams {
            bucket,
            engine: LocalEngine::Files,
        }),
        gossip: Arc::new(GossipTable::default()),
        self_announce: None,
        rate_limiter: std::sync::Arc::new(gitquarry_federation::rate_limit::PeerRateLimiter::default()),
        rate_limit_public_max: 60,
        rate_limit_public_window: std::time::Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: std::time::Duration::from_secs(60),
        rate_limit_peer_override_by_ip: true,
    })
}

async fn send(state: &Arc<FederationState>, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = router(Arc::clone(state)).oneshot(req).await.unwrap();
    let status = resp.status();
    (
        status,
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn banned_peers_lists_only_banned_ones_with_reason_and_actor() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let linked = Peer {
        name: "good-peer".to_string(),
        web_base_url: "https://good.example".to_string(),
        git_base_url: "https://good.example".to_string(),
        linked_at: String::new(),
        access_level: Default::default(),
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    let banned = Peer {
        name: "bad-peer".to_string(),
        web_base_url: "https://bad.example".to_string(),
        git_base_url: "https://bad.example".to_string(),
        linked_at: String::new(),
        access_level: Default::default(),
        banned: true,
        ban_reason: Some("spamming forks".to_string()),
        banned_at: Some("2026-08-06T00:00:00Z".to_string()),
        banned_by: "alice".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &linked, None)
        .await
        .unwrap();
    access::put_peer(state.backend.as_ref(), &state.cipher, &banned, None)
        .await
        .unwrap();

    let (status, bytes) = send(&state, get("/_api/banned-peers")).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let list = json.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["peer_name"], "bad-peer");
    assert_eq!(list[0]["ban_reason"], "spamming forks");
    assert_eq!(list[0]["banned_by"], "alice");
}

#[tokio::test]
async fn instance_get_reports_a_live_count_of_currently_banned_peers() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());

    let (_, bytes) = send(&state, get("/_api/instance")).await;
    let info: InstanceInfo = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(info.total_bans_issued, 0);

    for name in ["bad-1", "bad-2"] {
        let peer = Peer {
            name: name.to_string(),
            web_base_url: "https://bad.example".to_string(),
            git_base_url: "https://bad.example".to_string(),
            linked_at: String::new(),
            access_level: Default::default(),
            banned: true,
            ban_reason: Some("abuse".to_string()),
            banned_at: None,
            banned_by: "alice".to_string(),
            ..Default::default()
        };
        access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
            .await
            .unwrap();
    }

    let (_, bytes) = send(&state, get("/_api/instance")).await;
    let info: InstanceInfo = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(info.total_bans_issued, 2);
}
