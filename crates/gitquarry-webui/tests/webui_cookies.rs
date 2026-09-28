//! Cookie parsing and the attributes every `Set-Cookie` carries, observed
//! on the cookies the login flow really sets.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use gitquarry_core::app::AppContext;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery";

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[23u8; s3db_crypto::SALT_LEN]).unwrap());
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

/// A password user is what makes `/_login` render its form at all.
async fn app_with_user(root: &std::path::Path) -> axum::Router {
    let ctx = Arc::new(test_ctx(root));
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
    gitquarry_webui::webui::router(ctx)
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    app.clone().oneshot(request).await.unwrap()
}

async fn get(app: &axum::Router, path: &str, cookie: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::empty()).unwrap()).await
}

async fn post_form(app: &axum::Router, path: &str, cookie: Option<&str>, fields: &[(&str, &str)]) -> Response {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::from(body)).unwrap()).await
}

async fn body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The `value` of the first `name="csrf"` field, the way a browser would
/// submit it.
fn csrf_in(html: &str) -> Option<String> {
    let rest = &html[html.find(r#"name="csrf""#)?..];
    let rest = &rest[rest.find(r#"value=""#)? + r#"value=""#.len()..];
    Some(rest[..rest.find('"')?].to_string())
}

/// `name=value` of the `Set-Cookie` for `name`.
fn set_cookie_pair(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap_or_default().to_string())
}

/// `(login-csrf cookie pair, the token the form embeds)`.
async fn login_page(app: &axum::Router) -> (String, String) {
    let response = get(app, "/_login", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let anchor = set_cookie_pair(&response, "login-csrf").expect("no login-csrf cookie");
    let token = csrf_in(&body_text(response).await).expect("no csrf field on /_login");
    (anchor, token)
}

/// The `web-session=...` pair a real password login hands back.
async fn login(app: &axum::Router) -> String {
    let (anchor, token) = login_page(app).await;
    let response = post_form(
        app,
        "/_login",
        Some(&anchor),
        &[
            ("username", "alice"),
            ("password", PASSWORD),
            ("csrf", &token),
            ("return_to", "/"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    set_cookie_pair(&response, "web-session").expect("login set no session cookie")
}

/// The full `Set-Cookie` line for `name`, attributes included.
fn set_cookie_line(response: &Response, name: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no Set-Cookie for {name}"))
        .to_string()
}

async fn login_csrf_cookie_with_proto(app: &axum::Router, proto: Option<&str>) -> String {
    let mut builder = Request::builder().uri("/_login");
    if let Some(proto) = proto {
        builder = builder.header("x-forwarded-proto", proto);
    }
    let response = send(app, builder.body(Body::empty()).unwrap()).await;
    set_cookie_line(&response, "login-csrf")
}

/// Whether the session reached the page: `/_account/security` redirects to
/// `/_login` for anyone it can't find a session for.
async fn is_logged_in(app: &axum::Router, cookie: &str) -> bool {
    let response = get(app, "/_account/security", Some(cookie)).await;
    match response.status() {
        StatusCode::OK => true,
        StatusCode::SEE_OTHER => false,
        other => panic!("unexpected {other}"),
    }
}

#[tokio::test]
async fn the_session_cookie_is_found_among_several() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = login(&app).await;
    let value = session.strip_prefix("web-session=").unwrap();
    assert!(is_logged_in(&app, &format!("a=1; {session}; b=2")).await);
    assert!(is_logged_in(&app, &format!("{session};theme=dark")).await);
    // Matched by exact name, not by suffix or prefix.
    assert!(!is_logged_in(&app, &format!("old-web-session={value}")).await);
    assert!(!is_logged_in(&app, &format!("web-session-x={value}")).await);
    assert!(!is_logged_in(&app, "a=1; b=2").await);
}

#[tokio::test]
async fn the_session_cookie_is_http_only_same_site_and_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let (anchor, token) = login_page(&app).await;
    let response = post_form(
        &app,
        "/_login",
        Some(&anchor),
        &[("username", "alice"), ("password", PASSWORD), ("csrf", &token)],
    )
    .await;
    let line = set_cookie_line(&response, "web-session");
    assert!(line.contains("; Path=/"), "{line}");
    assert!(line.contains("; HttpOnly"), "{line}");
    assert!(line.contains("; SameSite=Lax"), "{line}");
    // `login::SESSION_TTL`: seven days.
    assert!(line.contains("; Max-Age=604800"), "{line}");
}

/// Plain HTTP must not get `Secure` — the browser would drop the cookie and
/// login would silently never stick.
#[tokio::test]
async fn a_plain_http_request_gets_no_secure_flag() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    for proto in [None, Some("http"), Some("http, https")] {
        let line = login_csrf_cookie_with_proto(&app, proto).await;
        assert!(!line.contains("Secure"), "{proto:?}: {line}");
        assert!(line.contains("HttpOnly") && line.contains("SameSite=Lax"), "{line}");
    }
}

/// Behind a TLS-terminating proxy it must be there, so the session cookie
/// never travels over plain HTTP. A proxy chain appends, and the first entry
/// is the hop that faced the client.
#[tokio::test]
async fn a_request_that_reached_the_proxy_over_https_gets_secure() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    for proto in ["https", "HTTPS", "https, http"] {
        let line = login_csrf_cookie_with_proto(&app, Some(proto)).await;
        assert!(line.contains("; Secure"), "{proto}: {line}");
    }
}

#[tokio::test]
async fn the_session_cookie_is_secure_behind_an_https_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let (anchor, token) = login_page(&app).await;
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("username", "alice"), ("password", PASSWORD), ("csrf", &token)])
        .finish();
    let request = Request::builder()
        .method("POST")
        .uri("/_login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, anchor)
        .header("x-forwarded-proto", "https")
        .body(Body::from(body))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let line = set_cookie_line(&response, "web-session");
    assert!(line.contains("; Secure"), "{line}");
}
