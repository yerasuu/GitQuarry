//! What a failed request tells the client: a raw route stays plain text even
//! when it fails, a 4xx explains itself, and a 5xx never quotes its cause.

use axum::http::{StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_test_support::{Page, Session};
use std::sync::Arc;

async fn get(ctx: AppContext, path: &str) -> Page {
    Session::get_as(&gitquarry_webui::webui::router(Arc::new(ctx)), path, None).await
}

fn content_type(page: &Page) -> &str {
    page.headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_failed_raw_request_is_plain_text_not_html() {
    let dir = tempfile::tempdir().unwrap();
    let page = get(gitquarry_test_support::ctx(dir.path()), "/_raw/plain/acme/missing").await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert_eq!(content_type(&page), "text/plain; charset=utf-8");
    assert!(!page.body.contains("<html"), "expected plain text, got: {}", page.body);
    assert!(page.body.contains("acme/missing"), "body: {}", page.body);
}

/// A 4xx is a message this codebase wrote for the reader, and still is one.
#[tokio::test]
async fn a_client_error_still_explains_itself() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    ctx.create_repo("acme/demo").await.unwrap().unwrap();
    let page = get(ctx, "/_raw/patch/acme/demo?sha=not-a-sha").await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST, "{}", page.body);
    assert_eq!(content_type(&page), "text/plain; charset=utf-8");
    assert!(
        page.body.starts_with("error: ") && page.body.contains("sha"),
        "no reason given: {}",
        page.body
    );
}

/// A 5xx must never hand the client the underlying anyhow chain — it
/// routinely quotes git's stderr, an S3 key, or a path on disk. A bucket
/// that is a plain file makes every backend read fail with an error naming
/// that path.
#[tokio::test]
async fn a_server_error_hides_the_underlying_chain_from_the_client() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("secret-bucket-path");
    std::fs::write(&bucket, b"not a directory").unwrap();
    for path in ["/acme/demo", "/_raw/plain/acme/demo"] {
        let ctx = AppContext {
            backend: gitquarry_test_support::backend(&bucket),
            ..gitquarry_test_support::ctx(dir.path())
        };
        let page = get(ctx, path).await;
        assert_eq!(
            page.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "GET {path}: {}",
            page.body
        );
        assert!(
            !page.body.contains("secret-bucket-path"),
            "GET {path} leaked a path: {}",
            page.body
        );
        assert!(
            page.body.contains("internal server error"),
            "GET {path}: no generic message: {}",
            page.body
        );
    }
}
