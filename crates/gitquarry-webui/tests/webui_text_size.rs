//! The about tab's text-size buttons and `/_prefs/text_size`, driven through
//! `webui::router` the way a browser reaches them.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_test_support::{Page, Session, ctx, send};
use std::sync::Arc;

/// A router over a fresh bucket holding one empty `acme/proj`, whose about
/// tab is where the text-size buttons render.
async fn app_with_repo(root: &std::path::Path) -> axum::Router {
    let ctx = ctx(root);
    ctx.create_repo("acme/proj").await.unwrap().unwrap();
    gitquarry_webui::webui::router(Arc::new(ctx))
}

async fn about_page(app: &axum::Router, cookie: Option<&str>) -> String {
    let page = Session::get_as(app, "/acme/proj?view=about", cookie).await;
    assert_eq!(page.status, StatusCode::OK);
    page.body
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

async fn toggle(app: &axum::Router, form: &str) -> Page {
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
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.headers[header::LOCATION], "/acme/proj");
    let set_cookie = response.headers[header::SET_COOKIE].to_str().unwrap();
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
        assert_eq!(response.status, StatusCode::SEE_OTHER);
        assert_eq!(response.headers[header::LOCATION], "/", "return_to={return_to}");
        assert!(
            response.headers[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .starts_with("text-size=md;")
        );
    }
}
