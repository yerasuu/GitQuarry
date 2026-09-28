//! Double-submit CSRF as a browser meets it: the token a page embeds, the
//! POST that has to echo it back, and every way of getting it wrong.

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
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[17u8; s3db_crypto::SALT_LEN]).unwrap());
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

/// The token a logged-in page hands this session.
async fn session_token(app: &axum::Router, session: &str) -> String {
    let html = body_text(get(app, "/_account/security", Some(session)).await).await;
    csrf_in(&html).expect("no csrf field on an authenticated page")
}

#[tokio::test]
async fn an_anonymous_page_embeds_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let html = body_text(get(&app, "/_license", None).await).await;
    assert!(csrf_in(&html).is_none(), "anonymous page carries a csrf field");
}

/// The token in the markup must be *derived* from the session, never the
/// session itself — every authenticated page carries this field, so
/// embedding the cookie would put a live credential in the DOM and undo
/// what `HttpOnly` is there for.
#[tokio::test]
async fn the_embedded_token_is_never_the_session_cookie_itself() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = login(&app).await;
    let session_value = session.strip_prefix("web-session=").unwrap();
    let html = body_text(get(&app, "/_account/security", Some(&session)).await).await;
    let token = csrf_in(&html).expect("no csrf field");
    assert!(!token.is_empty());
    assert_ne!(token, session_value);
    assert!(!html.contains(session_value), "session cookie leaked into the page");
}

#[tokio::test]
async fn a_post_carrying_this_sessions_token_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = login(&app).await;
    let token = session_token(&app, &session).await;
    let response = post_form(&app, "/_logout", Some(&session), &[("csrf", &token)]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");
    assert_eq!(
        set_cookie_pair(&response, "web-session").as_deref(),
        Some("web-session=")
    );
}

/// A token minted under one session must not validate under another.
#[tokio::test]
async fn another_sessions_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let mine = login(&app).await;
    let theirs = login(&app).await;
    assert_ne!(mine, theirs, "two logins produced the same session");
    let their_token = session_token(&app, &theirs).await;
    let response = post_form(&app, "/_logout", Some(&mine), &[("csrf", &their_token)]).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_pair(&response, "web-session").is_none());
}

#[tokio::test]
async fn a_wrong_missing_or_raw_cookie_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = login(&app).await;
    let session_value = session.strip_prefix("web-session=").unwrap().to_string();
    for token in ["wrong", "", session_value.as_str()] {
        let response = post_form(&app, "/_logout", Some(&session), &[("csrf", token)]).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "csrf={token:?}");
    }
    let response = post_form(&app, "/_logout", Some(&session), &[]).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "no csrf field at all");
    // No session cookie at all: nothing to compare against, always rejected.
    let token = session_token(&app, &session).await;
    let response = post_form(&app, "/_logout", None, &[("csrf", &token)]).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// `/_login` has no session to anchor on, so it carries its own cookie —
/// the same double-submit check, anchored on that one instead.
#[tokio::test]
async fn the_login_form_is_anchored_on_its_own_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let (anchor, token) = login_page(&app).await;
    let (other_anchor, other_token) = login_page(&app).await;
    let anchor_value = anchor.strip_prefix("login-csrf=").unwrap();
    let attempt = |cookie: Option<String>, csrf: String| {
        let app = app.clone();
        async move {
            post_form(
                &app,
                "/_login",
                cookie.as_deref(),
                &[("username", "alice"), ("password", PASSWORD), ("csrf", &csrf)],
            )
            .await
        }
    };

    // The anchor value itself is not the token.
    let response = attempt(Some(anchor.clone()), anchor_value.to_string()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Nor is a token derived from some other anchor.
    let response = attempt(Some(anchor.clone()), other_token).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // A missing anchor fails closed rather than accepting anything.
    let response = attempt(None, token.clone()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_pair(&response, "web-session").is_none());

    let response = attempt(Some(other_anchor), token.clone()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = attempt(Some(anchor), token).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(set_cookie_pair(&response, "web-session").is_some());
}
