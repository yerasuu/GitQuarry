//! The unified/split diff preference: which layout a page draws from the
//! `diff-view` cookie, the toggle buttons above the diff, and the
//! `POST /_prefs/diff_view` target that sets the cookie.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Page, Session, Work, ctx, send};
use std::sync::Arc;

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> String {
    let page = Session::get_as(app, uri, cookie).await;
    assert_eq!(page.status, StatusCode::OK, "{uri}: {}", page.body);
    page.body
}

/// One commit, pushed into a fresh public `team/proj` as a real pack, so
/// the commit page has a diff to draw from the server's own mirror.
async fn fixture() -> (tempfile::TempDir, Router, String) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    let sha = work.commit(&[("f.txt", "hello\n")], "initial").await;
    work.push(&repo, "refs/heads/main").await;
    (dir, gitquarry_webui::webui::router(ctx), sha)
}

async fn toggle(view: &str, return_to: &str) -> Page {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(ctx(dir.path())));
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("view", view), ("return_to", return_to)])
        .finish();
    let request = Request::post("/_prefs/diff_view")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    send(&app, request).await
}

fn is_split(html: &str) -> bool {
    let split = html.contains(r#"<table class="diff-split">"#);
    let unified = html.contains(r#"<span class="diff-hunk">"#);
    assert_ne!(split, unified, "exactly one layout should render: {html}");
    split
}

#[tokio::test]
async fn the_diff_is_unified_unless_the_cookie_picks_split() {
    let (_dir, app, sha) = fixture().await;
    let uri = format!("/team/proj?view=commit&sha={sha}");
    assert!(!is_split(&get(&app, &uri, None).await));
    assert!(is_split(&get(&app, &uri, Some("diff-view=split")).await));
    assert!(!is_split(&get(&app, &uri, Some("diff-view=nonsense")).await));
}

#[tokio::test]
async fn diff_view_buttons_mark_the_current_one_active() {
    let (_dir, app, sha) = fixture().await;
    let uri = format!("/team/proj?view=commit&sha={sha}");
    let html = get(&app, &uri, Some("diff-view=split")).await;
    let forms: Vec<&str> = html
        .split(r#"<form method="post" action="/_prefs/diff_view""#)
        .skip(1)
        .map(|rest| rest.split("</form>").next().unwrap())
        .collect();
    assert_eq!(forms.len(), 2, "{html}");
    let (unified, split) = (forms[0], forms[1]);
    assert!(unified.contains(r#"name="view" value="unified""#), "{unified}");
    assert!(split.contains(r#"name="view" value="split""#), "{split}");
    assert!(!unified.contains("size-btn active"), "{unified}");
    assert!(split.contains("size-btn active"), "{split}");
    // Each button brings the reader back to the page they flipped it on.
    let return_to = format!(r#"name="return_to" value="/team/proj?view=commit&amp;sha={sha}""#);
    assert!(forms.iter().all(|form| form.contains(&return_to)), "{html}");
}

#[tokio::test]
async fn toggle_diff_view_sets_the_explicit_pick_and_redirects() {
    let page = toggle("split", "/acme/proj").await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location(), "/acme/proj");
    let set_cookie = page.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("diff-view=split;"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");

    // Anything but the two known values is stored as the default.
    let page = toggle("sideways", "/").await;
    assert!(
        page.headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("diff-view=unified;")
    );
}

#[tokio::test]
async fn toggle_diff_view_rejects_an_off_site_return_to() {
    for return_to in ["//evil.example", "/\\evil.example", "https://evil.example", ""] {
        let page = toggle("unified", return_to).await;
        assert_eq!(page.status, StatusCode::SEE_OTHER);
        assert_eq!(page.location(), "/", "{return_to:?}");
    }
}
