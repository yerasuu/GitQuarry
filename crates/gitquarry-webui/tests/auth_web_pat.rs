//! `/_account/pat`: the logged-in account's personal access tokens, created
//! and revoked through `router()` with a real session.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_test_support::{Session, ctx, send, user};
use std::sync::Arc;

/// `/_account/pat` as `session` sees it, which has to render.
async fn pat_page(app: &axum::Router, session: &Session) -> String {
    let page = session.get(app, "/_account/pat").await;
    assert_eq!(page.status, StatusCode::OK, "GET /_account/pat");
    page.body
}

async fn app_with_alice(root: &std::path::Path) -> (axum::Router, Session) {
    let ctx = ctx(root);
    user(&ctx, "alice", false).await;
    let public = gitquarry_core::storage::entities::Visibility::Public;
    gitquarry_core::domain::repo::create(&ctx, "alice/proj", None, None, None, public)
        .await
        .unwrap()
        .unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(ctx));
    let session = Session::login(&app, "alice").await;
    (app, session)
}

/// The one-time secret out of the reveal banner's `<pre>`.
fn revealed_secret(html: &str) -> &str {
    let start = html.find("<pre>").unwrap_or_else(|| panic!("no secret in {html}")) + "<pre>".len();
    &html[start..start + html[start..].find("</pre>").unwrap()]
}

/// Regression: `revoke` (per-row) and `create` both target `#pat-body` —
/// either one has to refresh the whole wrapper, since the flash banner and
/// the one-time secret reveal sit above the table, outside either form.
/// Losing either form's `data-async` attributes silently brings back the
/// browser's "confirm resubmission" dialog on F5.
#[tokio::test]
async fn create_and_revoke_share_one_async_target() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let html = pat_page(&app, &session).await;
    assert!(html.contains("no tokens yet"), "{html}");
    assert_eq!(
        html.matches(r##"data-async data-async-target="#pat-body""##).count(),
        1,
        "{html}"
    );

    let fields = [
        ("csrf", session.csrf.as_str()),
        ("pat_action", "create"),
        ("label", "ci"),
        ("scopes", "alice/proj:developer"),
    ];
    // As `style/app.js` sends a `data-async` submit, which gets back just
    // the fragment its target names.
    let request = Request::post("/_account/pat")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish(),
        ))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::OK);
    let fragment = response.body;
    assert!(fragment.starts_with(r#"<div id="pat-body">"#), "{fragment}");
    // One token now listed, so its per-row `revoke` form renders too.
    assert_eq!(
        fragment
            .matches(r##"data-async data-async-target="#pat-body""##)
            .count(),
        2,
        "{fragment}"
    );
    assert!(fragment.contains(r#"value="revoke""#), "{fragment}");
    let secret = revealed_secret(&fragment).to_string();
    assert!(!secret.is_empty());

    // Shown once: a later visit lists the token but never its secret.
    let html = pat_page(&app, &session).await;
    assert!(html.contains("<td>ci</td>"), "{html}");
    assert!(html.contains("<code>alice/proj:developer</code>"), "{html}");
    assert!(!html.contains(&secret), "{html}");
}

#[tokio::test]
async fn revoking_removes_the_token_from_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;
    let fields = [
        ("pat_action", "create"),
        ("label", "ci"),
        ("scopes", "alice/proj:reporter"),
    ];
    session.post(&app, "/_account/pat", &fields).await;

    let html = pat_page(&app, &session).await;
    // A control named `action` would shadow the `data-async` form's own
    // `.action`, which `style/app.js` posts to.
    assert!(!html.contains(r#"name="action""#), "{html}");
    let marker = r#"name="uid" value=""#;
    let start = html.find(marker).unwrap() + marker.len();
    let uid = &html[start..start + html[start..].find('"').unwrap()];
    let response = session
        .post(&app, "/_account/pat", &[("pat_action", "revoke"), ("uid", uid)])
        .await;
    assert_eq!(response.status, StatusCode::OK);

    let html = pat_page(&app, &session).await;
    assert!(html.contains("no tokens yet"), "{html}");
}

/// A PAT can never scope to "every repo" — `*` is refused outright, not
/// looked up as a repo name.
#[tokio::test]
async fn a_wildcard_scope_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let fields = [("pat_action", "create"), ("label", "ci"), ("scopes", "*:owner")];
    let html = session.post(&app, "/_account/pat", &fields).await.body;
    assert!(html.contains("can't scope to all repos"), "{html}");
    assert!(html.contains("no tokens yet"), "{html}");
}

#[tokio::test]
async fn a_pat_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (app, session) = app_with_alice(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("pat_action", "create"),
        ("label", "ci"),
        ("scopes", "alice/proj:owner"),
    ];
    let request = Request::post("/_account/pat")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish(),
        ))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let html = pat_page(&app, &session).await;
    assert!(html.contains("no tokens yet"), "{html}");

    let response = Session::get_as(&app, "/_account/pat", None).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/_login?return_to=/_account/pat");
}
