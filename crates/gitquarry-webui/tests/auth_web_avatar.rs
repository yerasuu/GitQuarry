//! `/_account/profile`: the logged-in account's avatar and email addresses,
//! driven through `router()` with a real session.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_test_support::{Session, ctx, post_form, send, user};
use std::sync::Arc;

/// The page at `uri` for `session`, which must render.
async fn page(app: &axum::Router, session: &Session, uri: &str) -> String {
    let page = session.get(app, uri).await;
    assert_eq!(page.status, StatusCode::OK, "GET {uri}");
    page.body
}

async fn app_with_alice(root: &std::path::Path) -> (axum::Router, Session) {
    let ctx = ctx(root);
    user(&ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(Arc::new(ctx));
    let session = Session::login(&app, "alice").await;
    (app, session)
}

/// Regression: `set_email` targets `#primary-email-section`,
/// `add_secondary_email` (alongside `remove_secondary_email`) targets
/// `#secondary-emails-table` — neither can share the other's target, since
/// a page-level fragment swapped into either would corrupt it
/// (`style/app.js`'s `target.replaceWith` taking the whole `<html>` as the
/// replacement). Losing either attribute quietly brings back the browser's
/// "confirm resubmission" dialog on F5.
#[tokio::test]
async fn email_forms_target_their_own_section() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;
    page(&app, &session, "/_account/profile").await;

    let fields = [
        ("csrf", session.csrf.as_str()),
        ("email_action", "add_secondary_email"),
        ("secondary_email", "alt@example.com"),
    ];
    // `x-gq-async` because that is how `style/app.js` sends a `data-async`
    // submit, so only the fragment its target names comes back.
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(&fields)
        .finish();
    let request = Request::post("/_account/profile/emails")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(body))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::OK);
    let fragment = response.body;
    assert!(
        fragment.starts_with(r#"<div id="secondary-emails-table">"#),
        "{fragment}"
    );
    assert!(fragment.contains("alt@example.com"), "{fragment}");

    let fields = [
        ("csrf", session.csrf.as_str()),
        ("email_action", "set_email"),
        ("email", "alice@example.com"),
    ];
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(&fields)
        .finish();
    let request = Request::post("/_account/profile/emails")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(body))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::OK);
    let fragment = response.body;
    assert!(
        fragment.starts_with(r#"<div id="primary-email-section">"#),
        "{fragment}"
    );
    assert!(fragment.contains(r#"value="alice@example.com""#), "{fragment}");

    let html = page(&app, &session, "/_account/profile").await;
    assert!(
        html.contains(r##"data-async data-async-target="#primary-email-section""##),
        "{html}"
    );
    // The add form plus the one existing row's remove form.
    assert_eq!(
        html.matches(r##"data-async data-async-target="#secondary-emails-table""##)
            .count(),
        2,
        "{html}"
    );
}

#[tokio::test]
async fn an_email_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("email_action", "set_email"),
        ("email", "mallory@example.com"),
    ];
    let response = post_form(&app, "/_account/profile/emails", Some(&session.cookie), &fields).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let html = page(&app, &session, "/_account/profile").await;
    assert!(!html.contains("mallory@example.com"), "{html}");

    let response = Session::get_as(&app, "/_account/profile", None).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/_login?return_to=/_account/profile");
}
