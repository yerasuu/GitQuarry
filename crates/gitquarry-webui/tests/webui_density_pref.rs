//! The topbar's dense/airy toggle and `/_prefs/density`, driven through
//! `webui::router` the way a browser reaches them.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_test_support::{Page, Session, ctx, send};
use std::sync::Arc;

/// A router over a fresh bucket holding one empty `acme/proj`: its about
/// tab is a real page rendered through `layout`'s topbar.
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

async fn toggle(app: &axum::Router, referer: &str) -> Page {
    let request = Request::post("/_prefs/density")
        .header(header::HOST, "gitquarry.example")
        .header(header::REFERER, referer)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("density=airy"))
        .unwrap();
    send(app, request).await
}

#[tokio::test]
async fn html_data_density_follows_the_explicit_cookie_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    assert!(
        about_page(&app, Some("density=dense"))
            .await
            .contains(r#"data-density="dense""#)
    );
    assert!(
        about_page(&app, Some("density=airy"))
            .await
            .contains(r#"data-density="airy""#)
    );
    // No cookie, or a garbage value: no attribute, so the `:root` default
    // applies.
    assert!(!about_page(&app, None).await.contains(r#"data-density=""#));
    assert!(
        !about_page(&app, Some("density=roomy"))
            .await
            .contains(r#"data-density=""#)
    );
}

#[tokio::test]
async fn toggle_marks_the_current_pick_active() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let html = about_page(&app, Some("density=airy")).await;
    assert_eq!(html.matches("density-btn active").count(), 1);
    assert!(html.contains(r#"value="airy" class="density-btn active""#));
}

#[tokio::test]
async fn no_pick_reads_as_dense_not_as_neither() {
    // Unlike theme, density has no OS-level signal: no pick is the `:root`
    // default, which is dense, so the dense button is the one that should
    // look active.
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let html = about_page(&app, None).await;
    assert_eq!(html.matches("density-btn active").count(), 1);
    assert!(html.contains(r#"value="dense" class="density-btn active""#));
}

#[tokio::test]
async fn toggle_sets_the_explicit_pick_and_redirects_to_a_same_host_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let page = toggle(&app, "https://gitquarry.example/acme/proj").await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location(), "/acme/proj");
    let set_cookie = page.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("density=airy;"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
}

#[tokio::test]
async fn toggle_never_follows_a_foreign_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let page = toggle(&app, "https://evil.example/acme/proj").await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location(), "/");
}
