//! `/_healthz` and `/_metrics`, as a probe or scraper sees them through
//! `webui::router`: unauthenticated GETs, no session.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[16u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// Content type and body of an anonymous `GET path`, which must be a 200.
async fn get(ctx: &Arc<AppContext>, path: &str) -> (String, String) {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = gitquarry_webui::webui::router(Arc::clone(ctx))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "GET {path}");
    let content_type = response.headers()[header::CONTENT_TYPE].to_str().unwrap().to_string();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (content_type, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn healthz_is_a_plain_ok() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    let (content_type, body) = get(&ctx, "/_healthz").await;
    assert_eq!(content_type, "text/plain; charset=utf-8");
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn metrics_includes_every_gauge_with_its_value() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
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
    let ctx = Arc::new(test_ctx(dir.path()));
    let (_, body) = get(&ctx, "/_metrics").await;
    assert!(body.contains("\ngitquarry_login_penalised_active 0\n"), "{body}");
    assert!(body.contains("\ngitquarry_git_rate_limited_active 0\n"), "{body}");
}
