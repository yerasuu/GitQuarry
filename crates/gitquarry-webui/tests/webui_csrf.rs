//! Double-submit CSRF as a browser meets it: the token a page embeds, the
//! POST that has to echo it back, and every way of getting it wrong.

use axum::http::{StatusCode, header};
use gitquarry_test_support::{PASSWORD, Page, Session, post_form};
use std::sync::Arc;

/// A password user is what makes `/_login` render its form at all.
async fn app_with_user(root: &std::path::Path) -> axum::Router {
    let ctx = Arc::new(gitquarry_test_support::ctx(root));
    gitquarry_test_support::user(&ctx, "alice", false).await;
    gitquarry_webui::webui::router(ctx)
}

/// Whether `page` plants a `name` cookie at all; `Page::cookie` panics
/// when it doesn't.
fn sets_cookie(page: &Page, name: &str) -> bool {
    page.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.starts_with(&format!("{name}=")))
}

/// `(login-csrf cookie pair, the token the form embeds)`.
async fn login_page(app: &axum::Router) -> (String, String) {
    let page = Session::get_as(app, "/_login", None).await;
    assert_eq!(page.status, StatusCode::OK);
    (page.cookie("login-csrf"), page.csrf())
}

#[tokio::test]
async fn an_anonymous_page_embeds_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let page = Session::get_as(&app, "/_license", None).await;
    assert!(
        !page.body.contains(r#"name="csrf""#),
        "anonymous page carries a csrf field"
    );
}

/// The token in the markup must be *derived* from the session, never the
/// session itself — every authenticated page carries this field, so
/// embedding the cookie would put a live credential in the DOM and undo
/// what `HttpOnly` is there for.
#[tokio::test]
async fn the_embedded_token_is_never_the_session_cookie_itself() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = Session::login(&app, "alice").await;
    let session_value = session.cookie.strip_prefix("web-session=").unwrap();
    let page = session.get(&app, "/_account/security").await;
    let token = page.csrf();
    assert!(!token.is_empty());
    assert_ne!(token, session_value);
    assert!(
        !page.body.contains(session_value),
        "session cookie leaked into the page"
    );
}

#[tokio::test]
async fn a_post_carrying_this_sessions_token_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = Session::login(&app, "alice").await;
    let response = session.post(&app, "/_logout", &[]).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/");
    assert_eq!(response.cookie("web-session"), "web-session=");
}

/// A token minted under one session must not validate under another.
#[tokio::test]
async fn another_sessions_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let mine = Session::login(&app, "alice").await;
    let theirs = Session::login(&app, "alice").await;
    assert_ne!(mine.cookie, theirs.cookie, "two logins produced the same session");
    let response = post_form(&app, "/_logout", Some(&mine.cookie), &[("csrf", &theirs.csrf)]).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(!sets_cookie(&response, "web-session"));
}

#[tokio::test]
async fn a_wrong_missing_or_raw_cookie_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_user(dir.path()).await;
    let session = Session::login(&app, "alice").await;
    let session_value = session.cookie.strip_prefix("web-session=").unwrap();
    for token in ["wrong", "", session_value] {
        let response = post_form(&app, "/_logout", Some(&session.cookie), &[("csrf", token)]).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "csrf={token:?}");
    }
    let response = post_form(&app, "/_logout", Some(&session.cookie), &[]).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST, "no csrf field at all");
    // No session cookie at all: nothing to compare against, always rejected.
    let response = post_form(&app, "/_logout", None, &[("csrf", &session.csrf)]).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
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
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    // Nor is a token derived from some other anchor.
    let response = attempt(Some(anchor.clone()), other_token).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    // A missing anchor fails closed rather than accepting anything.
    let response = attempt(None, token.clone()).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(!sets_cookie(&response, "web-session"));

    let response = attempt(Some(other_anchor), token.clone()).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);

    let response = attempt(Some(anchor), token).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert!(sets_cookie(&response, "web-session"));
}
