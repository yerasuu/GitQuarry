use gitquarry_federation::web::*;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_federation::state::FederationState;
use gitquarry_test_support::{federation_state, send};
use std::sync::Arc;

fn with_public_max(root: &std::path::Path, public_max: u32) -> Arc<FederationState> {
    Arc::new(FederationState {
        rate_limit_public_max: public_max,
        ..(*federation_state(root, "http://127.0.0.1:1")).clone()
    })
}

#[tokio::test]
async fn public_routes_429_past_the_configured_ceiling_but_only_for_that_caller() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(with_public_max(dir.path(), 1));

    let get_instance = async |ip: &str| {
        let req = Request::get("/_api/instance")
            .header("x-forwarded-for", ip)
            .body(Body::empty())
            .unwrap();
        send(&app, req).await.status
    };

    assert_eq!(get_instance("1.2.3.4").await, StatusCode::OK);
    assert_eq!(get_instance("1.2.3.4").await, StatusCode::TOO_MANY_REQUESTS);
    // A different caller IP has its own independent allowance.
    assert_eq!(get_instance("5.6.7.8").await, StatusCode::OK);
}

#[tokio::test]
async fn other_routes_are_never_rate_limited() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(with_public_max(dir.path(), 0));
    let req = Request::get("/_internal-out/peers")
        .header("x-forwarded-for", "1.2.3.4")
        .body(Body::empty())
        .unwrap();
    // `rate_limit_public_max: 0` would 429 this if it were on the
    // public-routes layer — it isn't, so the request reaches the
    // handler and gets a normal response instead.
    assert_eq!(send(&app, req).await.status, StatusCode::OK);
}
