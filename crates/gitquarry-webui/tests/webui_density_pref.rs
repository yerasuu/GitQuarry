//! The topbar's dense/airy toggle and `/_prefs/density`, driven through
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

async fn toggle(app: &axum::Router, referer: &str) -> Response {
    let request = Request::post("/_prefs/density")
        .header(header::HOST, "gitquarry.example")
        .header(header::REFERER, referer)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("density=airy"))
        .unwrap();
    send(app, request).await
}

#[tokio::test]
async fn html_data_density_follows_the_explicit_cookie_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    assert!(
        about_page(&app, Some("density=dense"))
            .await
            .contains(r#"data-density="dense""#)
    );
    assert!(
        about_page(&app, Some("density=airy"))
            .await
            .contains(r#"data-density="airy""#)
    );
    // No cookie, or a garbage value: no attribute, so the `:root` default
    // applies.
    assert!(!about_page(&app, None).await.contains(r#"data-density=""#));
    assert!(
        !about_page(&app, Some("density=roomy"))
            .await
            .contains(r#"data-density=""#)
    );
}

#[tokio::test]
async fn toggle_marks_the_current_pick_active() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let html = about_page(&app, Some("density=airy")).await;
    assert_eq!(html.matches("density-btn active").count(), 1);
    assert!(html.contains(r#"value="airy" class="density-btn active""#));
}

#[tokio::test]
async fn no_pick_reads_as_dense_not_as_neither() {
    // Unlike theme, density has no OS-level signal: no pick is the `:root`
    // default, which is dense, so the dense button is the one that should
    // look active.
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let html = about_page(&app, None).await;
    assert_eq!(html.matches("density-btn active").count(), 1);
    assert!(html.contains(r#"value="dense" class="density-btn active""#));
}

#[tokio::test]
async fn toggle_sets_the_explicit_pick_and_redirects_to_a_same_host_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let response = toggle(&app, "https://gitquarry.example/acme/proj").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/acme/proj");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("density=airy;"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
}

#[tokio::test]
async fn toggle_never_follows_a_foreign_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let response = toggle(&app, "https://evil.example/acme/proj").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");
}
