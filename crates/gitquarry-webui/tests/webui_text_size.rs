use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_webui::webui::text_size::*;
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
        mirror_sync: None,
    }
}

#[test]
fn cookie_text_size_reads_explicit_overrides_only() {
    let mut large = HeaderMap::new();
    large.insert(axum::http::header::COOKIE, "text-size=lg".parse().unwrap());
    assert_eq!(cookie_text_size(&large), Some(TextSize::Lg));

    let mut small = HeaderMap::new();
    small.insert(axum::http::header::COOKIE, "text-size=sm".parse().unwrap());
    assert_eq!(cookie_text_size(&small), Some(TextSize::Sm));

    // Absent (no session override yet) is deliberately `None` —
    // callers fall back to the account default first.
    assert_eq!(cookie_text_size(&HeaderMap::new()), None);
}

#[test]
fn text_size_buttons_mark_the_current_one_active() {
    let html = render_text_size_buttons("/acme/proj", TextSize::Md);
    assert_eq!(html.matches("size-btn active").count(), 1);
    assert!(html.contains(r#"value="sm""#));
    assert!(html.contains(r#"value="md""#));
    assert!(html.contains(r#"value="lg""#));
    // The active one is specifically the medium button, not just any.
    let md_form = html.split("<form").nth(2).unwrap();
    assert!(md_form.contains("active"));
}

#[test]
fn text_size_buttons_return_to_this_summary() {
    let html = render_text_size_buttons("/acme/proj?rev=main", TextSize::Sm);
    assert!(html.contains(r#"action="/_prefs/text_size""#));
    assert!(html.contains("acme/proj"));
}

#[tokio::test]
async fn toggle_text_size_sets_the_explicit_pick_and_redirects() {
    let form = Form(TextSizeForm {
        size: "lg".to_string(),
        return_to: "/acme/proj".to_string(),
    });
    let dir = tempfile::tempdir().unwrap();
    let resp = handler_toggle_text_size(State(Arc::new(test_ctx(dir.path()))), HeaderMap::new(), form).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get(axum::http::header::LOCATION).unwrap();
    assert_eq!(location, "/acme/proj");
    let set_cookie = resp
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("text-size=lg"));
}

#[tokio::test]
async fn toggle_text_size_rejects_an_off_site_return_to() {
    let form = Form(TextSizeForm {
        size: "md".to_string(),
        return_to: "//evil.example".to_string(),
    });
    let dir = tempfile::tempdir().unwrap();
    let resp = handler_toggle_text_size(State(Arc::new(test_ctx(dir.path()))), HeaderMap::new(), form).await;
    let location = resp.headers().get(axum::http::header::LOCATION).unwrap();
    assert_eq!(location, "/");
}

#[test]
fn theme_and_text_size_toggles_carry_an_aria_label() {
    assert!(
        gitquarry_webui::webui::theme_pref::render_theme_toggle("light")
            .into_string()
            .contains(r#"aria-label="light mode""#)
    );
    let sizes = render_text_size_buttons("/acme/proj", TextSize::Md);
    assert!(sizes.contains(r#"aria-label="small""#));
    assert!(sizes.contains(r#"aria-label="medium""#));
    assert!(sizes.contains(r#"aria-label="large""#));
}
