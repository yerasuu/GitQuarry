//! The `locale` cookie picks the language `layout`'s own chrome renders in.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

/// Same shape as `webui_routes.rs`'s own `test_ctx`, duplicated because
/// every file under `tests/` is its own crate.
fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
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

/// A router over a fresh bucket holding one empty `acme/proj`: its about
/// tab is a real page rendered through `layout`'s topbar.
async fn app_with_repo(root: &std::path::Path) -> axum::Router {
    let ctx = test_ctx(root);
    ctx.create_repo("acme/proj").await.unwrap().unwrap();
    gitquarry_webui::webui::router(Arc::new(ctx))
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> axum::response::Response {
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't add it, and its absence 500s before routing.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    app.clone().oneshot(request).await.unwrap()
}

async fn about_page(app: &axum::Router, cookie: Option<&str>) -> String {
    let mut request = Request::get("/acme/proj?view=about");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let response = send(app, request.body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn an_explicit_locale_cookie_translates_the_chrome() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let es = about_page(&app, Some("locale=es")).await;
    assert!(es.contains("saltar al contenido"));
    assert!(!es.contains("skip to content"));

    assert!(about_page(&app, Some("locale=en")).await.contains("skip to content"));
}

#[tokio::test]
async fn a_missing_or_unknown_locale_falls_back_to_english() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    for cookie in [None, Some("locale=fr")] {
        let html = about_page(&app, cookie).await;
        assert!(html.contains("skip to content"), "cookie={cookie:?}");
        assert!(!html.contains("saltar al contenido"), "cookie={cookie:?}");
    }
}
