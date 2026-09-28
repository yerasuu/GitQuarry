//! What a failed request tells the client: a raw route stays plain text even
//! when it fails, a 4xx explains itself, and a 5xx never quotes its cause.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

fn test_ctx(root: &Path, bucket: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: bucket.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[13u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn get(ctx: AppContext, path: &str) -> (StatusCode, String, String) {
    let app = gitquarry_webui::webui::router(Arc::new(ctx));
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, content_type, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn a_failed_raw_request_is_plain_text_not_html() {
    let dir = tempfile::tempdir().unwrap();
    let (status, content_type, body) = get(test_ctx(dir.path(), dir.path()), "/_raw/plain/acme/missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(content_type, "text/plain; charset=utf-8");
    assert!(!body.contains("<html"), "expected plain text, got: {body}");
    assert!(body.contains("acme/missing"), "body: {body}");
}

/// A 4xx is a message this codebase wrote for the reader, and still is one.
#[tokio::test]
async fn a_client_error_still_explains_itself() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), dir.path());
    ctx.create_repo("acme/demo").await.unwrap().unwrap();
    let (status, content_type, body) = get(ctx, "/_raw/patch/acme/demo?sha=not-a-sha").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(content_type, "text/plain; charset=utf-8");
    assert!(
        body.starts_with("error: ") && body.contains("sha"),
        "no reason given: {body}"
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
        let (status, _content_type, body) = get(test_ctx(dir.path(), &bucket), path).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "GET {path}: {body}");
        assert!(!body.contains("secret-bucket-path"), "GET {path} leaked a path: {body}");
        assert!(
            body.contains("internal server error"),
            "GET {path}: no generic message: {body}"
        );
    }
}
