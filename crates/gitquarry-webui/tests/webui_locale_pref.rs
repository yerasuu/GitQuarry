//! The `locale` cookie picks the language `layout`'s own chrome renders in.

use axum::http::StatusCode;
use gitquarry_test_support::{Session, ctx};
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

#[tokio::test]
async fn an_explicit_locale_cookie_translates_the_chrome() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    let es = about_page(&app, Some("locale=es")).await;
    assert!(es.contains("saltar al contenido"));
    assert!(!es.contains("skip to content"));

    assert!(about_page(&app, Some("locale=en")).await.contains("skip to content"));
}

#[tokio::test]
async fn a_missing_or_unknown_locale_falls_back_to_english() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_repo(dir.path()).await;

    for cookie in [None, Some("locale=fr")] {
        let html = about_page(&app, cookie).await;
        assert!(html.contains("skip to content"), "cookie={cookie:?}");
        assert!(!html.contains("saltar al contenido"), "cookie={cookie:?}");
    }
}
