//! The tab icon: one asset route, and the `<link>` every page's `<head>`
//! carries for it. Both together, since either one alone is silently
//! useless — a served icon nothing links to, or a link to a 404.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use gitquarry_core::app::AppContext;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }),
    );
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[7u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
    );
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
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn get(app: axum::Router, path: &str) -> (StatusCode, String, String) {
    let request = Request::builder()
        .uri(path)
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1234))))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, content_type, String::from_utf8_lossy(&body).to_string())
}

#[tokio::test]
async fn the_favicon_route_serves_the_brand_mark_as_svg() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(dir.path())));

    let (status, content_type, body) = get(app, "/_assets/favicon.svg").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "image/svg+xml");
    assert!(body.starts_with("<svg "), "{body}");
    // The same mark the topbar draws, viewBox included -- a path lifted
    // into a different coordinate system would render as a blank tab.
    assert!(body.contains(r#"viewBox="0 0 24 24""#), "{body}");
    assert!(body.contains("M12 1.5 2 7v10l10 5.5L22 17V7L12 1.5Z"), "{body}");
}

#[tokio::test]
async fn every_page_head_links_the_favicon() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(dir.path())));

    let (status, _content_type, body) = get(app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"<link rel="icon" type="image/svg+xml" href="/_assets/favicon.svg">"#),
        "{body}"
    );
}
