//! `/_account/appearance`: the logged-in account's display defaults, set
//! and read back through `router()` with a real session.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[SALT_BYTE; s3db_crypto::SALT_LEN]).unwrap(),
    );
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

async fn create_user(ctx: &AppContext, username: &str, is_admin: bool) {
    domain::user::create(
        ctx,
        domain::user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't add it, and its absence 500s before routing.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    app.clone().oneshot(request).await.unwrap()
}

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::get(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn post_form(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

/// [`post_form`] as `style/app.js` sends a `data-async` submit, which gets
/// back just the fragment its target names.
fn post_async(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let mut request = post_form(uri, cookie, fields);
    request.headers_mut().insert("x-gq-async", "1".parse().unwrap());
    request
}

async fn text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// `name=value` out of the response's `Set-Cookie`, the shape a browser
/// sends back in `Cookie`.
fn cookie_pair(response: &Response, name: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .and_then(|v| v.split(';').next())
        .unwrap_or_else(|| panic!("no Set-Cookie for {name}"))
        .to_string()
}

fn csrf_in(html: &str) -> String {
    let marker = r#"name="csrf" value=""#;
    let start = html.find(marker).unwrap_or_else(|| panic!("no csrf field in {html}")) + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

/// A real password login, returning the session cookie.
async fn log_in(app: &axum::Router, username: &str) -> String {
    let response = send(app, get("/_login", "")).await;
    let anchor = cookie_pair(&response, "login-csrf");
    let csrf = csrf_in(&text(response).await);
    let fields = [("csrf", csrf.as_str()), ("username", username), ("password", PASSWORD)];
    let response = send(app, post_form("/_login", &anchor, &fields)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    cookie_pair(&response, "web-session")
}

/// The page at `uri` for `cookie`'s session, plus the CSRF token its forms
/// carry — read out of the markup the way a browser submits it.
async fn page(app: &axum::Router, uri: &str, cookie: &str) -> (String, String) {
    let response = send(app, get(uri, cookie)).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
    let html = text(response).await;
    let csrf = csrf_in(&html);
    (html, csrf)
}

fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

const SALT_BYTE: u8 = 12;

async fn app_with_alice(root: &std::path::Path) -> (axum::Router, String) {
    let ctx = test_ctx(root);
    create_user(&ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(Arc::new(ctx));
    let session = log_in(&app, "alice").await;
    (app, session)
}

async fn save_pref(app: &axum::Router, session: &str, action: &str, field: &str, value: &str) -> String {
    let (_html, csrf) = page(app, "/_account/appearance", session).await;
    let response = send(
        app,
        post_form(
            "/_account/appearance",
            session,
            &[("csrf", &csrf), ("action", action), (field, value)],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    text(response).await
}

#[tokio::test]
async fn the_page_needs_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    create_user(&ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(Arc::new(ctx));

    let response = send(&app, get("/_account/appearance", "")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/_login?return_to=/_account/appearance");
}

#[tokio::test]
async fn a_pref_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let fields = [("csrf", "forged"), ("action", "set_theme_pref"), ("theme", "dark")];
    let response = send(&app, post_form("/_account/appearance", &session, &fields)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(html.contains(r#"<option value="" selected>automatic (follows the system)</option>"#));
}

#[tokio::test]
async fn text_size_pref_shows_the_saved_default_as_selected() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    save_pref(&app, &session, "set_text_size_pref", "text_size", "sm").await;
    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(html.contains(r#"<option value="sm" selected>small</option>"#), "{html}");
    assert!(html.contains(r#"<option value="md">medium</option>"#));
    assert!(html.contains(r#"<option value="lg">large</option>"#));

    save_pref(&app, &session, "set_text_size_pref", "text_size", "lg").await;
    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(html.contains(r#"<option value="lg" selected>large</option>"#), "{html}");
}

#[tokio::test]
async fn theme_pref_shows_the_saved_default_as_selected() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(html.contains(r#"<option value="" selected>automatic (follows the system)</option>"#));
    assert!(html.contains(r#"<option value="light">light</option>"#));
    assert!(html.contains(r#"<option value="dark">dark</option>"#));

    save_pref(&app, &session, "set_theme_pref", "theme", "dark").await;
    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(
        html.contains(r#"<option value="dark" selected>dark</option>"#),
        "{html}"
    );
}

#[tokio::test]
async fn locale_pref_shows_the_saved_default_as_selected() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(html.contains(r#"<option value="" selected>automatic (uses the site's built-in default)</option>"#));
    assert!(html.contains(r#"<option value="es">Español</option>"#));
    assert!(html.contains(r#"<option value="en">English</option>"#));

    save_pref(&app, &session, "set_locale_pref", "locale", "es").await;
    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(
        html.contains(r#"<option value="es" selected>Español</option>"#),
        "{html}"
    );
}

#[tokio::test]
async fn an_unknown_pref_value_is_rejected_and_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let (_html, csrf) = page(&app, "/_account/appearance", &session).await;
    let fields = [("csrf", csrf.as_str()), ("action", "set_theme_pref"), ("theme", "neon")];
    let response = send(&app, post_form("/_account/appearance", &session, &fields)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let (html, _csrf) = page(&app, "/_account/appearance", &session).await;
    assert!(html.contains(r#"<option value="" selected>automatic (follows the system)</option>"#));
}

/// All 4 preference forms share one `#appearance-body` async target so a
/// submit refreshes the shared flash banner above them, not just its own
/// form. Regression: a form that quietly loses `data-async`/its target
/// brings back the "confirm resubmission" dialog on F5.
#[tokio::test]
async fn every_pref_form_shares_one_async_target_and_gets_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let (html, csrf) = page(&app, "/_account/appearance", &session).await;
    assert_eq!(
        html.matches(r##"data-async data-async-target="#appearance-body""##)
            .count(),
        4,
        "{html}"
    );

    let fields = [
        ("csrf", csrf.as_str()),
        ("action", "set_density_pref"),
        ("density", "airy"),
    ];
    let response = send(&app, post_async("/_account/appearance", &session, &fields)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let fragment = text(response).await;
    assert!(fragment.starts_with(r#"<div id="appearance-body">"#), "{fragment}");
    assert!(fragment.contains(r#"<option value="airy" selected>"#), "{fragment}");
}
