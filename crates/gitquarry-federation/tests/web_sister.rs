use gitquarry_federation::web::sister::*;

use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::gossip::GossipTable;
use gitquarry_federation::rate_limit::PeerRateLimiter;
use gitquarry_federation::sister::SisterBanEntry;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::{access, sister};
use s3db::{BackendConfig, LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};

fn test_state(root: &std::path::Path) -> Arc<FederationState> {
    let bucket = root.to_string_lossy().to_string();
    let backend = LocalBackend::new(&LocalParams {
        bucket: bucket.clone(),
        engine: LocalEngine::Redb,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[77u8; s3db_crypto::SALT_LEN]).unwrap();
    Arc::new(FederationState {
        backend: Arc::new(backend),
        cipher: Arc::new(cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
        backend_config: BackendConfig::Local(LocalParams {
            bucket,
            engine: LocalEngine::Redb,
        }),
        gossip: Arc::new(GossipTable::default()),
        self_announce: None,
        rate_limiter: Arc::new(PeerRateLimiter::default()),
        rate_limit_public_max: 60,
        rate_limit_public_window: Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: Duration::from_secs(60),
        rate_limit_peer_override_by_ip: true,
    })
}

#[tokio::test]
async fn handshake_confirms_only_once_this_side_already_requested() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();

    let body = serde_json::to_vec(&serde_json::json!({ "instance_name": "acme", "secret": "shh" })).unwrap();
    let resp = handler_handshake_post(State(Arc::clone(&state)), Bytes::from(body.clone())).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["ok"], false);

    sister::request(state.backend.as_ref(), &state.cipher, "acme")
        .await
        .unwrap();
    let resp = handler_handshake_post(State(Arc::clone(&state)), Bytes::from(body)).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["ok"], true);
}

#[tokio::test]
async fn receive_handshake_rejects_a_different_secret_once_already_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
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
    let state = test_state(dir.path());
    let resp = handler_ban_event_post(State(Arc::clone(&state)), HeaderMap::new(), Bytes::new()).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = handler_bans_get(State(state), HeaderMap::new()).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ban_event_applies_and_bans_get_reflects_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
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

    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, "Bearer shared-secret".parse().unwrap());
    let event = serde_json::to_vec(&serde_json::json!({
        "peer_name": "stranger", "git_base_url": "https://stranger.example",
        "action": "ban", "reason": "abuse", "banned_by": "bob", "banned_at": null
    }))
    .unwrap();
    let resp = handler_ban_event_post(State(Arc::clone(&state)), headers.clone(), Bytes::from(event)).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = handler_bans_get(State(state), headers).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let list: Vec<SisterBanEntry> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].peer_name, "stranger");
}
