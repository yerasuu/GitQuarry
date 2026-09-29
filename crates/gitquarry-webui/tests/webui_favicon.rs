//! The tab icon: one asset route, and the `<link>` every page's `<head>`
//! carries for it. Both together, since either one alone is silently
//! useless — a served icon nothing links to, or a link to a 404.

use axum::http::{StatusCode, header};
use gitquarry_test_support::{Session, ctx};
use std::sync::Arc;

#[tokio::test]
async fn the_favicon_route_serves_the_brand_mark_as_svg() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(ctx(dir.path())));

    let page = Session::get_as(&app, "/_assets/favicon.svg", None).await;

    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.headers.get(header::CONTENT_TYPE).unwrap(), "image/svg+xml");
    let body = &page.body;
    assert!(body.starts_with("<svg "), "{body}");
    // The same mark the topbar draws, viewBox included -- a path lifted
    // into a different coordinate system would render as a blank tab.
    assert!(body.contains(r#"viewBox="0 0 24 24""#), "{body}");
    assert!(body.contains("M12 1.5 2 7v10l10 5.5L22 17V7L12 1.5Z"), "{body}");
}

#[tokio::test]
async fn every_page_head_links_the_favicon() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(ctx(dir.path())));

    let page = Session::get_as(&app, "/", None).await;

    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body
            .contains(r#"<link rel="icon" type="image/svg+xml" href="/_assets/favicon.svg">"#),
        "{}",
        page.body
    );
}
