//! `/_account/appearance`: the logged-in account's display defaults, set
//! and read back through `router()` with a real session.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_test_support::{Session, ctx, send, user};
use std::sync::Arc;

async fn app_with_alice(root: &std::path::Path) -> (axum::Router, Session) {
    let ctx = ctx(root);
    user(&ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(Arc::new(ctx));
    let session = Session::login(&app, "alice").await;
    (app, session)
}

/// The page at `uri` for `session`, which must render.
async fn page(app: &axum::Router, session: &Session, uri: &str) -> String {
    let page = session.get(app, uri).await;
    assert_eq!(page.status, StatusCode::OK, "GET {uri}");
    page.body
}

async fn save_pref(app: &axum::Router, session: &Session, action: &str, field: &str, value: &str) -> String {
    let page = session
        .post(
            app,
            "/_account/appearance",
            &[("appearance_action", action), (field, value)],
        )
        .await;
    assert_eq!(page.status, StatusCode::OK);
    page.body
}

/// A control named `action` shadows its form's `.action` property, which
/// `style/app.js` posts every `data-async` form to.
#[tokio::test]
async fn no_form_control_shadows_the_form_action() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;
    let html = page(&app, &session, "/_account/appearance").await;
    assert!(html.contains("data-async"), "{html}");
    assert!(!html.contains(r#"name="action""#), "{html}");
}

#[tokio::test]
async fn the_page_needs_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    user(&ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(Arc::new(ctx));

    let page = Session::get_as(&app, "/_account/appearance", None).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location(), "/_login?return_to=/_account/appearance");
}

#[tokio::test]
async fn a_pref_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("appearance_action", "set_theme_pref"),
        ("theme", "dark"),
    ];
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let request = Request::post("/_account/appearance")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    assert_eq!(send(&app, request).await.status, StatusCode::BAD_REQUEST);
    let html = page(&app, &session, "/_account/appearance").await;
    assert!(html.contains(r#"<option value="" selected>automatic (follows the system)</option>"#));
}

#[tokio::test]
async fn text_size_pref_shows_the_saved_default_as_selected() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    save_pref(&app, &session, "set_text_size_pref", "text_size", "sm").await;
    let html = page(&app, &session, "/_account/appearance").await;
    assert!(html.contains(r#"<option value="sm" selected>small</option>"#), "{html}");
    assert!(html.contains(r#"<option value="md">medium</option>"#));
    assert!(html.contains(r#"<option value="lg">large</option>"#));

    save_pref(&app, &session, "set_text_size_pref", "text_size", "lg").await;
    let html = page(&app, &session, "/_account/appearance").await;
    assert!(html.contains(r#"<option value="lg" selected>large</option>"#), "{html}");
}

#[tokio::test]
async fn theme_pref_shows_the_saved_default_as_selected() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let html = page(&app, &session, "/_account/appearance").await;
    assert!(html.contains(r#"<option value="" selected>automatic (follows the system)</option>"#));
    assert!(html.contains(r#"<option value="light">light</option>"#));
    assert!(html.contains(r#"<option value="dark">dark</option>"#));

    save_pref(&app, &session, "set_theme_pref", "theme", "dark").await;
    let html = page(&app, &session, "/_account/appearance").await;
    assert!(
        html.contains(r#"<option value="dark" selected>dark</option>"#),
        "{html}"
    );
}

#[tokio::test]
async fn locale_pref_shows_the_saved_default_as_selected() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let html = page(&app, &session, "/_account/appearance").await;
    assert!(html.contains(r#"<option value="" selected>automatic (uses the site's built-in default)</option>"#));
    assert!(html.contains(r#"<option value="es">Español</option>"#));
    assert!(html.contains(r#"<option value="en">English</option>"#));

    save_pref(&app, &session, "set_locale_pref", "locale", "es").await;
    let html = page(&app, &session, "/_account/appearance").await;
    assert!(
        html.contains(r#"<option value="es" selected>Español</option>"#),
        "{html}"
    );
}

#[tokio::test]
async fn an_unknown_pref_value_is_rejected_and_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let fields = [("appearance_action", "set_theme_pref"), ("theme", "neon")];
    let response = session.post(&app, "/_account/appearance", &fields).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let html = page(&app, &session, "/_account/appearance").await;
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

    let html = page(&app, &session, "/_account/appearance").await;
    assert_eq!(
        html.matches(r##"data-async data-async-target="#appearance-body""##)
            .count(),
        4,
        "{html}"
    );

    let fields = [
        ("csrf", session.csrf.as_str()),
        ("appearance_action", "set_density_pref"),
        ("density", "airy"),
    ];
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    // `style/app.js` marks a `data-async` submit this way, which gets back
    // just the fragment its target names.
    let request = Request::post("/_account/appearance")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(body))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::OK);
    let fragment = response.body;
    assert!(fragment.starts_with(r#"<div id="appearance-body">"#), "{fragment}");
    assert!(fragment.contains(r#"<option value="airy" selected>"#), "{fragment}");
}
