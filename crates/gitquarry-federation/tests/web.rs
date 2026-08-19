use gitquarry_federation::web::*;

use axum::Router;
use axum::http::StatusCode;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::gossip::GossipTable;
use gitquarry_federation::rate_limit::PeerRateLimiter;
use gitquarry_federation::state::FederationState;
use s3db::{BackendConfig, LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;
use std::time::Duration;

fn test_state(root: &std::path::Path, public_max: u32) -> Arc<FederationState> {
    let bucket = root.to_string_lossy().to_string();
    let backend = LocalBackend::new(&LocalParams {
        bucket: bucket.clone(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[55u8; s3db_crypto::SALT_LEN]).unwrap();
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
        rate_limiter: Arc::new(PeerRateLimiter::default()),
        rate_limit_public_max: public_max,
        rate_limit_public_window: Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: Duration::from_secs(60),
        rate_limit_peer_override_by_ip: true,
    })
}

#[tokio::test]
async fn public_routes_429_past_the_configured_ceiling_but_only_for_that_caller() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), 1);
    let app = router(state);

    let get_instance = |app: Router, ip: &'static str| {
        let app = app.clone();
        async move {
            use tower::ServiceExt;
            app.oneshot(
                axum::http::Request::get("/_api/instance")
                    .header("x-forwarded-for", ip)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    let resp = get_instance(app.clone(), "1.2.3.4").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = get_instance(app.clone(), "1.2.3.4").await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    // A different caller IP has its own independent allowance.
    let resp = get_instance(app.clone(), "5.6.7.8").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn other_routes_are_never_rate_limited() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), 0);
    let app = router(state);
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::get("/_internal-out/peers")
                .header("x-forwarded-for", "1.2.3.4")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // `rate_limit_public_max: 0` would 429 this if it were on the
    // public-routes layer — it isn't, so the request reaches the
    // handler and gets a normal response instead.
    assert_eq!(resp.status(), StatusCode::OK);
}
