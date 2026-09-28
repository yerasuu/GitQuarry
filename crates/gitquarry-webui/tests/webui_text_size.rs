//! The about tab's text-size buttons and `/_prefs/text_size`, driven through
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

/// A router over a fresh bucket holding one empty `acme/proj`, whose about
/// tab is where the text-size buttons render.
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

/// The `size` hidden input of whichever of the three forms carries the
/// `active` button.
fn active_size(html: &str) -> Vec<&str> {
    html.split(r#"action="/_prefs/text_size""#)
        .skip(1)
        .filter(|form| form.split("</form>").next().unwrap().contains("size-btn active"))
        .map(|form| {
            let value = form.split(r#"name="size" value=""#).nth(1).unwrap();
            &value[..value.find('"').unwrap()]
        })
        .collect()
}

async fn toggle(app: &axum::Router, form: &str) -> Response {
    let request = Request::post("/_prefs/text_size")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .unwrap();
    send(app, request).await
}

#[tokio::test]
async fn the_active_button_follows_the_explicit_cookie_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    assert_eq!(active_size(&about_page(&app, Some("text-size=lg")).await), ["lg"]);
    assert_eq!(active_size(&about_page(&app, Some("text-size=md")).await), ["md"]);
    // No override and no account default: the original small size, not
    // whatever `body` would cascade.
    assert_eq!(active_size(&about_page(&app, None).await), ["sm"]);
    assert_eq!(active_size(&about_page(&app, Some("text-size=huge")).await), ["sm"]);
}

#[tokio::test]
async fn every_button_posts_back_to_this_about_tab() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;
    let html = about_page(&app, None).await;

    assert_eq!(html.matches(r#"action="/_prefs/text_size""#).count(), 3);
    for size in ["sm", "md", "lg"] {
        assert!(html.contains(&format!(r#"name="size" value="{size}""#)));
    }
    assert_eq!(
        html.matches(r#"name="return_to" value="/acme/proj?view=about""#)
            .count(),
        3
    );
}

#[tokio::test]
async fn theme_and_text_size_toggles_carry_an_aria_label() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;
    let html = about_page(&app, None).await;

    assert!(html.contains(r#"aria-label="light mode""#));
    assert!(html.contains(r#"aria-label="small""#));
    assert!(html.contains(r#"aria-label="medium""#));
    assert!(html.contains(r#"aria-label="large""#));
}

#[tokio::test]
async fn toggle_sets_the_explicit_pick_and_redirects_back() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let response = toggle(&app, "size=lg&return_to=%2Facme%2Fproj").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/acme/proj");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("text-size=lg;"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");
}

#[tokio::test]
async fn toggle_never_redirects_off_site() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    for return_to in [
        "%2F%2Fevil.example",
        "https%3A%2F%2Fevil.example%2F",
        "%2F%5Cevil.example",
    ] {
        let response = toggle(&app, &format!("size=md&return_to={return_to}")).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/", "return_to={return_to}");
        assert!(
            response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .starts_with("text-size=md;")
        );
    }
}
