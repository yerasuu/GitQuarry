use gitquarry_webui::webui::error::*;

use axum::body::to_bytes;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;

#[tokio::test]
async fn into_plain_response_is_plain_text_not_html() {
    let err = AppError::not_found(anyhow::anyhow!("no such path"));
    let resp = err.into_plain_response();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains("<html"), "expected plain text, got: {body}");
    assert!(body.contains("no such path"), "body: {body}");
}

/// A 5xx must never hand the client the underlying anyhow chain — it
/// routinely quotes git's stderr, an S3 key, or a mirror path on disk.
#[tokio::test]
async fn a_server_error_hides_the_underlying_chain_from_the_client() {
    let err = AppError::from(anyhow::anyhow!(
        "could not open /srv/mirrors/secret-repo.git: bucket gq-prod"
    ));
    let resp = err.into_response();
    assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(!body.contains("/srv/mirrors"), "leaked a filesystem path: {body}");
    assert!(!body.contains("gq-prod"), "leaked a bucket name: {body}");
    assert!(body.contains("internal server error"), "no generic message: {body}");
}

/// A 4xx is a message this codebase wrote for the reader, and still is one.
#[tokio::test]
async fn a_client_error_still_explains_itself() {
    let err = AppError::not_found(anyhow::anyhow!("invalid rev (disallowed characters)"));
    let resp = err.into_plain_response();
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("invalid rev (disallowed characters)"), "{body}");
}
