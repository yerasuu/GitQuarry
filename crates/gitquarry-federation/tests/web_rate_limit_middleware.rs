//! The public-route rate limit as a caller meets it: requests through
//! `web::router`, keyed on the `X-Forwarded-For` core's proxy sets.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_federation::access;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::gossip::GossipTable;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use s3db::{BackendConfig, LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;
use tower::ServiceExt;

fn test_state(root: &std::path::Path, peer_override_by_ip: bool) -> Arc<FederationState> {
    let bucket = root.to_string_lossy().to_string();
    let backend = LocalBackend::new(&LocalParams {
        bucket: bucket.clone(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[71u8; s3db_crypto::SALT_LEN]).unwrap();
    Arc::new(FederationState {
        backend: std::sync::Arc::new(backend),
        cipher: std::sync::Arc::new(cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
        backend_config: BackendConfig::Local(LocalParams {
            bucket,
            engine: LocalEngine::Files,
        }),
        gossip: std::sync::Arc::new(GossipTable::default()),
        self_announce: None,
        rate_limiter: std::sync::Arc::new(gitquarry_federation::rate_limit::PeerRateLimiter::default()),
        rate_limit_public_max: 1,
        rate_limit_public_window: std::time::Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: std::time::Duration::from_secs(60),
        rate_limit_peer_override_by_ip: peer_override_by_ip,
    })
}

/// `true` unless the rate limit turned the request away.
async fn allowed(state: &Arc<FederationState>, ip: &str, bearer: Option<&str>) -> bool {
    let mut req = Request::get("/_api/instance").header("x-forwarded-for", ip);
    if let Some(bearer) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let resp = router(Arc::clone(state))
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    resp.status() != StatusCode::TOO_MANY_REQUESTS
}

async fn seed_boosted_peer(state: &Arc<FederationState>) {
    let peer = Peer {
        name: "acme".to_string(),
        // Matched on host alone: scheme, port and path are stripped.
        web_base_url: "https://1.2.3.4:8443/gitquarry".to_string(),
        git_base_url: "ssh://1.2.3.4:2222".to_string(),
        rate_limit_override: Some(100),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn peer_override_applies_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), true);
    seed_boosted_peer(&state).await;
    // Instance ceiling is 1, but the matching peer's override is 100 —
    // both calls succeed only if the override actually applied.
    assert!(allowed(&state, "1.2.3.4", None).await);
    assert!(allowed(&state, "1.2.3.4", None).await);
}

#[tokio::test]
async fn peer_override_disabled_falls_back_to_the_instance_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), false);
    seed_boosted_peer(&state).await;
    // Same boosted peer, same IP, but the toggle is off: capped at the
    // instance-wide ceiling (1) regardless.
    assert!(allowed(&state, "1.2.3.4", None).await);
    assert!(!allowed(&state, "1.2.3.4", None).await);
}

#[tokio::test]
async fn a_bearer_that_resolves_to_no_token_is_still_held_to_the_public_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), false);
    assert!(allowed(&state, "5.6.7.8", Some("token_garbage")).await);
    assert!(!allowed(&state, "5.6.7.8", Some("token_garbage")).await);
    // The ceiling is per caller, not global.
    assert!(allowed(&state, "9.9.9.9", None).await);
}
