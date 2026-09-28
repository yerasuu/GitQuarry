use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_webui::webui::theme_pref::*;
use std::sync::Arc;

/// Same shape as `webui_repo.rs`'s own `test_ctx`, duplicated for the same
/// reason (sibling test modules, no natural common parent).
fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
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
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

#[test]
fn cookie_theme_reads_explicit_overrides_only() {
    let mut dark = HeaderMap::new();
    dark.insert(axum::http::header::COOKIE, "theme=dark".parse().unwrap());
    assert_eq!(cookie_theme(&dark), Some("dark"));

    let mut light = HeaderMap::new();
    light.insert(axum::http::header::COOKIE, "theme=light".parse().unwrap());
    assert_eq!(cookie_theme(&light), Some("light"));

    // No cookie, or a garbage value: `None`, same as an absent
    // `text-size` cookie — callers fall back to the account default.
    assert_eq!(cookie_theme(&HeaderMap::new()), None);
    let mut garbage = HeaderMap::new();
    garbage.insert(axum::http::header::COOKIE, "theme=purple".parse().unwrap());
    assert_eq!(cookie_theme(&garbage), None);
}

#[test]
fn referer_path_accepts_same_host_paths_only() {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::HOST, "gitquarry.example".parse().unwrap());
    headers.insert(
        axum::http::header::REFERER,
        "https://gitquarry.example/acme/proj?view=log".parse().unwrap(),
    );
    assert_eq!(referer_path(&headers), "/acme/proj?view=log");

    // Different host in the Referer: exactly the open-redirect
    // `safe_return_to` already guards against for `return_to` fields.
    let mut cross_host = HeaderMap::new();
    cross_host.insert(axum::http::header::HOST, "gitquarry.example".parse().unwrap());
    cross_host.insert(
        axum::http::header::REFERER,
        "https://evil.example/acme/proj".parse().unwrap(),
    );
    assert_eq!(referer_path(&cross_host), "/");

    // No Referer at all (stripped by browser/extension, or a raw client):
    // falls back to `/` rather than erroring.
    let mut none = HeaderMap::new();
    none.insert(axum::http::header::HOST, "gitquarry.example".parse().unwrap());
    assert_eq!(referer_path(&none), "/");
}

#[tokio::test]
async fn toggle_theme_sets_the_explicit_pick_and_redirects_to_referer() {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::HOST, "gitquarry.example".parse().unwrap());
    headers.insert(
        axum::http::header::REFERER,
        "https://gitquarry.example/acme/proj".parse().unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let resp = handler_toggle_theme(
        State(Arc::new(test_ctx(dir.path()))),
        headers,
        Form(ThemeForm {
            theme: "dark".to_string(),
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
    assert!(set_cookie.contains("theme=dark"));
}

#[test]
fn theme_toggle_marks_the_current_pick_active() {
    let html = render_theme_toggle("dark").into_string();
    assert_eq!(html.matches("theme-btn active").count(), 1);
    let dark_button = html.split("value=\"dark\"").nth(1).unwrap();
    assert!(dark_button.starts_with(" class=\"theme-btn active\""));
}
