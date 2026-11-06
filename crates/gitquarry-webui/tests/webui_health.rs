//! `/_healthz` and `/_metrics`, as a probe or scraper sees them through
//! `webui::router`: unauthenticated GETs, no session.

use axum::http::{StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_test_support::{Session, ctx};
use std::sync::Arc;

/// Content type and body of an anonymous `GET path`, which must be a 200.
async fn get(ctx: &Arc<AppContext>, path: &str) -> (String, String) {
    let page = Session::get_as(&gitquarry_webui::webui::router(Arc::clone(ctx)), path, None).await;
    assert_eq!(page.status, StatusCode::OK, "GET {path}");
    let content_type = page.headers[header::CONTENT_TYPE].to_str().unwrap().to_string();
    (content_type, page.body)
}

#[tokio::test]
async fn healthz_is_a_plain_ok() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let (content_type, body) = get(&ctx, "/_healthz").await;
    assert_eq!(content_type, "text/plain; charset=utf-8");
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn metrics_includes_every_gauge_with_its_value() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    // Driven to the limiters' own thresholds rather than a hardcoded count,
    // so the gauges read exactly the number of keys pushed past them.
    for username in ["alice", "bob", "carol"] {
        while ctx.login_attempts.penalty(username).is_zero() {
            ctx.login_attempts.record_failure(username);
        }
    }
    for ip in ["192.0.2.1", "192.0.2.2"] {
        while ctx.git_rate_limiter.check_anonymous(ip) {}
    }

    let (content_type, body) = get(&ctx, "/_metrics").await;
    assert_eq!(content_type, "text/plain; version=0.0.4");
    assert!(body.contains("\ngitquarry_up 1\n"), "{body}");
    assert!(body.contains("\ngitquarry_uptime_seconds "), "{body}");
    assert!(body.contains("\ngitquarry_login_penalised_active 3\n"), "{body}");
    assert!(body.contains("\ngitquarry_git_rate_limited_active 2\n"), "{body}");
    // No version/build label: an unauthenticated endpoint shouldn't hand
    // out the one field that only helps someone matching the instance to a
    // known bug.
    assert!(!body.contains("gitquarry_build_info"), "{body}");
    assert!(!body.contains(env!("CARGO_PKG_VERSION")), "{body}");
}

#[tokio::test]
async fn metrics_zero_lockouts_still_shows_the_gauge() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let (_, body) = get(&ctx, "/_metrics").await;
    assert!(body.contains("\ngitquarry_login_penalised_active 0\n"), "{body}");
    assert!(body.contains("\ngitquarry_git_rate_limited_active 0\n"), "{body}");
}
