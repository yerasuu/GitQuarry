//! Cookie parsing and the attributes every `Set-Cookie` carries, observed
//! on the cookies the login flow really sets.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_test_support::{PASSWORD, Page, Session, send};
use std::sync::Arc;

/// A password user is what makes `/_login` render its form at all.
async fn app_with_user(root: &std::path::Path) -> axum::Router {
    let ctx = Arc::new(gitquarry_test_support::ctx(root));
    gitquarry_test_support::user(&ctx, "alice", false).await;
    gitquarry_webui::webui::router(ctx)
}

async fn post_form(app: &axum::Router, path: &str, cookie: Option<&str>, fields: &[(&str, &str)]) -> Page {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let mut builder = Request::post(path).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::from(body)).unwrap()).await
}

/// `(login-csrf cookie pair, the token the form embeds)`.
async fn login_page(app: &axum::Router) -> (String, String) {
    let page = Session::get_as(app, "/_login", None).await;
    assert_eq!(page.status, StatusCode::OK);
    (page.cookie("login-csrf"), page.csrf())
}

/// The full `Set-Cookie` line for `name`, attributes included.
fn set_cookie_line(page: &Page, name: &str) -> String {
    page.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no Set-Cookie for {name}"))
        .to_string()
}

async fn login_csrf_cookie_with_proto(app: &axum::Router, proto: Option<&str>) -> String {
    let mut builder = Request::get("/_login");
    if let Some(proto) = proto {
        builder = builder.header("x-forwarded-proto", proto);
    }
    let page = send(app, builder.body(Body::empty()).unwrap()).await;
    set_cookie_line(&page, "login-csrf")
}

/// Whether the session reached the page: `/_account/security` redirects to
/// `/_login` for anyone it can't find a session for.
async fn is_logged_in(app: &axum::Router, cookie: &str) -> bool {
    match Session::get_as(app, "/_account/security", Some(cookie)).await.status {
        StatusCode::OK => true,
        StatusCode::SEE_OTHER => false,
        other => panic!("unexpected {other}"),
    }
}

#[tokio::test]
async fn the_session_cookie_is_found_among_several() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = Session::login(&app, "alice").await.cookie;
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
    let request = Request::post("/_login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, anchor)
        .header("x-forwarded-proto", "https")
        .body(Body::from(body))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    let line = set_cookie_line(&response, "web-session");
    assert!(line.contains("; Secure"), "{line}");
}
