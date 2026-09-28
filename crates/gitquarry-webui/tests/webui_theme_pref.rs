//! The topbar's light/dark toggle and `/_prefs/theme`, driven through
//! `webui::router` the way a browser reaches them.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
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

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
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

async fn toggle(app: &axum::Router, referer: Option<&str>) -> Response {
    let mut request = Request::post("/_prefs/theme")
        .header(header::HOST, "gitquarry.example")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(referer) = referer {
        request = request.header(header::REFERER, referer);
    }
    send(app, request.body(Body::from("theme=dark")).unwrap()).await
}

#[tokio::test]
async fn html_data_theme_follows_the_explicit_cookie_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    assert!(
        about_page(&app, Some("theme=dark"))
            .await
            .contains(r#"data-theme="dark""#)
    );
    assert!(
        about_page(&app, Some("theme=light"))
            .await
            .contains(r#"data-theme="light""#)
    );
    // No cookie, or a garbage value: no attribute at all, so
    // `prefers-color-scheme` still decides.
    assert!(!about_page(&app, None).await.contains(r#"data-theme=""#));
    assert!(!about_page(&app, Some("theme=purple")).await.contains(r#"data-theme=""#));
}

#[tokio::test]
async fn toggle_marks_the_current_pick_active() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let html = about_page(&app, Some("theme=dark")).await;
    assert_eq!(html.matches("theme-btn active").count(), 1);
    assert!(html.contains(r#"value="dark" class="theme-btn active""#));

    // No explicit pick yet: neither button, since following the OS is a real
    // state of its own.
    assert_eq!(about_page(&app, None).await.matches("theme-btn active").count(), 0);
}

#[tokio::test]
async fn toggle_sets_the_explicit_pick_and_redirects_to_a_same_host_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let response = toggle(&app, Some("https://gitquarry.example/acme/proj?view=log")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/acme/proj?view=log");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("theme=dark;"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
}

#[tokio::test]
async fn toggle_never_follows_a_foreign_or_missing_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    // A cross-host Referer is exactly the open redirect `return_to` fields
    // are already guarded against; the host-prefix and `//` cases are the
    // two ways a naive prefix match would still let one through.
    for referer in [
        Some("https://evil.example/acme/proj"),
        Some("https://gitquarry.example.evil.example/acme/proj"),
        Some("https://gitquarry.example//evil.example"),
        None,
    ] {
        let response = toggle(&app, referer).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/", "referer={referer:?}");
    }
}
