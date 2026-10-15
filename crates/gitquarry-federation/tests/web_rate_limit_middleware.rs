//! The public-route rate limit as a caller meets it: requests through
//! `web::router`, keyed on the `X-Forwarded-For` core's proxy sets.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_federation::access;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use gitquarry_test_support::{federation_state, send};
use std::sync::Arc;

fn test_state(root: &std::path::Path, peer_override_by_ip: bool) -> Arc<FederationState> {
    Arc::new(FederationState {
        rate_limit_public_max: 1,
        rate_limit_peer_override_by_ip: peer_override_by_ip,
        ..Arc::unwrap_or_clone(federation_state(root, "http://127.0.0.1:1"))
    })
}

/// `true` unless the rate limit turned the request away.
async fn allowed(state: &Arc<FederationState>, ip: &str, bearer: Option<&str>) -> bool {
    let mut req = Request::get("/_api/instance").header("x-forwarded-for", ip);
    if let Some(bearer) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    send(&router(Arc::clone(state)), req.body(Body::empty()).unwrap())
        .await
        .status
        != StatusCode::TOO_MANY_REQUESTS
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
