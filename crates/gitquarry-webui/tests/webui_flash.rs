//! The flash a rejected mutation parks on its redirect: it has to reach the
//! page the browser lands on, and it has to be gone by the next one.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, header};
use gitquarry_core::app::AppContext;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Files,
        }),
    );
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[3u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
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
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        artifacts: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn get_with_cookie(root: &std::path::Path, path: &str, cookie: Option<&str>) -> (String, Vec<String>) {
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(root)));
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.oneshot(request).await.unwrap();
    let set_cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect();
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (String::from_utf8_lossy(&body).into_owned(), set_cookies)
}

#[tokio::test]
async fn a_parked_message_renders_on_the_page_landed_on() {
    let dir = tempfile::tempdir().unwrap();
    let (body, _) = get_with_cookie(dir.path(), "/_license", Some("gq_flash=e%27x%27+isn%27t+a+branch")).await;
    assert!(body.contains("'x' isn't a branch"), "flash missing from the page");
    assert!(
        body.contains(r#"class="alert flash flash-err""#),
        "flash rendered without its error styling"
    );
}

#[tokio::test]
async fn the_message_is_cleared_by_the_response_that_showed_it() {
    let dir = tempfile::tempdir().unwrap();
    let (_, set_cookies) = get_with_cookie(dir.path(), "/_license", Some("gq_flash=eboom")).await;
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
    let (body, set_cookies) = get_with_cookie(dir.path(), "/_license", None).await;
    // `flash-err` on its own also appears in the inlined stylesheet, so the
    // absence being checked is the markup's. `/_license` is a page with no
    // flash of its own -- `/_login`, say, renders one for a server without
    // OIDC configured.
    assert!(!body.contains(r#"class="alert flash flash-err""#));
    assert!(!set_cookies.iter().any(|c| c.starts_with("gq_flash=")));
}
