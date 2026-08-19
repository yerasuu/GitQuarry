use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_webui::webui::density_pref::*;
use std::sync::Arc;

/// Same shape as `webui_repo.rs`'s own `test_ctx`, duplicated for the same
/// reason (sibling test modules, no natural common parent).
fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Files,
        }),
    );
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
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
        mirror_max_age: std::time::Duration::from_secs(1),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        events: Default::default(),
    }
}

#[test]
fn cookie_density_reads_explicit_overrides_only() {
    let mut dense = HeaderMap::new();
    dense.insert(axum::http::header::COOKIE, "density=dense".parse().unwrap());
    assert_eq!(cookie_density(&dense), Some("dense"));

    let mut airy = HeaderMap::new();
    airy.insert(axum::http::header::COOKIE, "density=airy".parse().unwrap());
    assert_eq!(cookie_density(&airy), Some("airy"));

    // No cookie, or a garbage value: `None`, same as an absent
    // `theme`/`text-size` cookie — callers fall back to the account default.
    assert_eq!(cookie_density(&HeaderMap::new()), None);
    let mut garbage = HeaderMap::new();
    garbage.insert(axum::http::header::COOKIE, "density=roomy".parse().unwrap());
    assert_eq!(cookie_density(&garbage), None);
}

#[tokio::test]
async fn toggle_density_sets_the_explicit_pick_and_redirects_to_referer() {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::HOST, "gitquarry.example".parse().unwrap());
    headers.insert(
        axum::http::header::REFERER,
        "https://gitquarry.example/acme/proj".parse().unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let resp = handler_toggle_density(
        State(Arc::new(test_ctx(dir.path()))),
        headers,
        Form(DensityForm {
            density: "airy".to_string(),
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get(axum::http::header::LOCATION).unwrap();
    assert_eq!(location, "/acme/proj");
    let set_cookie = resp
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("density=airy"));
}

#[test]
fn density_toggle_marks_the_current_pick_active() {
    let html = render_density_toggle("airy").into_string();
    assert_eq!(html.matches("density-btn active").count(), 1);
    let airy_button = html.split("value=\"airy\"").nth(1).unwrap();
    assert!(airy_button.starts_with(" class=\"density-btn active\""));
}

#[test]
fn empty_density_reads_as_dense_not_as_no_pick() {
    // Unlike theme (whose empty state highlights neither button — a real
    // "follow the OS" state exists), density has no OS-level signal: `""`
    // is the actual rendered state (the `:root` default), so the dense
    // button should be the one visually active, not neither.
    let html = render_density_toggle("").into_string();
    assert_eq!(html.matches("density-btn active").count(), 1);
    let dense_button = html.split("value=\"dense\"").nth(1).unwrap();
    assert!(dense_button.starts_with(" class=\"density-btn active\""));
}
