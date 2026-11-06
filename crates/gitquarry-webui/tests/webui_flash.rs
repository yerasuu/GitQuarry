//! The flash a rejected mutation parks on its redirect: it has to reach the
//! page the browser lands on, and it has to be gone by the next one.

use axum::http::header;
use gitquarry_test_support::{Page, Session, ctx};
use std::sync::Arc;

async fn get_with_cookie(root: &std::path::Path, path: &str, cookie: Option<&str>) -> Page {
    let app = gitquarry_webui::webui::router(Arc::new(ctx(root)));
    Session::get_as(&app, path, cookie).await
}

fn set_cookies(page: &Page) -> Vec<&str> {
    page.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect()
}

#[tokio::test]
async fn a_parked_message_renders_on_the_page_landed_on() {
    let dir = tempfile::tempdir().unwrap();
    let body = get_with_cookie(dir.path(), "/_license", Some("gq_flash=e%27x%27+isn%27t+a+branch"))
        .await
        .body;
    assert!(body.contains("'x' isn't a branch"), "flash missing from the page");
    assert!(
        body.contains(r#"class="alert flash flash-err""#),
        "flash rendered without its error styling"
    );
}

#[tokio::test]
async fn the_message_is_cleared_by_the_response_that_showed_it() {
    let dir = tempfile::tempdir().unwrap();
    let page = get_with_cookie(dir.path(), "/_license", Some("gq_flash=eboom")).await;
    let set_cookies = set_cookies(&page);
    assert!(
        set_cookies
            .iter()
            .any(|c| c.starts_with("gq_flash=;") && c.contains("Max-Age=0")),
        "expected the flash cookie to be cleared, got {set_cookies:?}"
    );
}

#[tokio::test]
async fn a_page_without_a_flash_neither_renders_nor_clears_one() {
    let dir = tempfile::tempdir().unwrap();
    let page = get_with_cookie(dir.path(), "/_license", None).await;
    let (body, set_cookies) = (&page.body, set_cookies(&page));
    // `flash-err` on its own also appears in the inlined stylesheet, so the
    // absence being checked is the markup's. `/_license` is a page with no
    // flash of its own -- `/_login`, say, renders one for a server without
    // OIDC configured.
    assert!(!body.contains(r#"class="alert flash flash-err""#));
    assert!(!set_cookies.iter().any(|c| c.starts_with("gq_flash=")));
}
