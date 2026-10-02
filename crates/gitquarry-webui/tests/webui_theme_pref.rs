//! The topbar's light/dark toggle and `/_prefs/theme`, driven through
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

async fn toggle(app: &axum::Router, referer: Option<&str>) -> Page {
    let mut request = Request::post("/_prefs/theme")
        .header(header::HOST, "gitquarry.example")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(referer) = referer {
        request = request.header(header::REFERER, referer);
    }
    send(app, request.body(Body::from("theme=dark")).unwrap()).await
}

#[tokio::test]
async fn html_data_theme_follows_the_explicit_cookie_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    assert!(
        about_page(&app, Some("theme=dark"))
            .await
            .contains(r#"data-theme="dark""#)
    );
    assert!(
        about_page(&app, Some("theme=light"))
            .await
            .contains(r#"data-theme="light""#)
    );
    // No cookie, or a garbage value: no attribute at all, so
    // `prefers-color-scheme` still decides.
    assert!(!about_page(&app, None).await.contains(r#"data-theme=""#));
    assert!(!about_page(&app, Some("theme=purple")).await.contains(r#"data-theme=""#));
}

#[tokio::test]
async fn toggle_marks_the_current_pick_active() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let html = about_page(&app, Some("theme=dark")).await;
    assert_eq!(html.matches("theme-btn active").count(), 1);
    assert!(html.contains(r#"value="dark" class="theme-btn active""#));

    // No explicit pick yet: neither button, since following the OS is a real
    // state of its own.
    assert_eq!(about_page(&app, None).await.matches("theme-btn active").count(), 0);
}

#[tokio::test]
async fn toggle_sets_the_explicit_pick_and_redirects_to_a_same_host_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let response = toggle(&app, Some("https://gitquarry.example/acme/proj?view=log")).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.headers[header::LOCATION], "/acme/proj?view=log");
    let set_cookie = response.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("theme=dark;"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
}

#[tokio::test]
async fn toggle_never_follows_a_foreign_or_missing_referer() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    // A cross-host Referer is exactly the open redirect `return_to` fields
    // are already guarded against; the host-prefix and `//` cases are the
    // two ways a naive prefix match would still let one through.
    for referer in [
        Some("https://evil.example/acme/proj"),
        Some("https://gitquarry.example.evil.example/acme/proj"),
        Some("https://gitquarry.example//evil.example"),
        None,
    ] {
        let response = toggle(&app, referer).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER);
        assert_eq!(response.headers[header::LOCATION], "/", "referer={referer:?}");
    }
}
