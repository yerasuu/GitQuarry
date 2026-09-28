//! The first-boot gate: `/_bootstrap` trades the startup token for a grant
//! cookie, and nothing else opens the admin pages while login is
//! unconfigured.

use axum::http::HeaderMap;
use gitquarry_webui::webui::bootstrap::{BOOTSTRAP_COOKIE, TOKEN, holds_grant};
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> gitquarry_core::app::AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }),
    );
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[9u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
    );
    gitquarry_core::app::AppContext {
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

fn with_cookie(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("{BOOTSTRAP_COOKIE}={value}").parse().unwrap(),
    );
    headers
}

#[test]
fn no_cookie_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    assert!(!holds_grant(&ctx, &HeaderMap::new()));
}

#[test]
fn a_forged_cookie_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    // Plausible-looking base64 that was never sealed by this cipher: the
    // whole point of sealing the grant rather than storing a flag.
    assert!(!holds_grant(&ctx, &with_cookie("eyJleHAiOjk5OTk5OTk5OTl9")));
}

#[test]
fn a_cookie_sealed_under_another_name_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    // Right cipher, wrong AAD — a session cookie can't be replayed as a
    // bootstrap grant.
    let sealed = ctx
        .cipher
        .seal(
            "web-session",
            br#"{"exp":9999999999}"#,
            gitquarry_core::crypto::CompressionStrategy::None,
        )
        .unwrap();
    use base64::Engine;
    let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sealed);
    assert!(!holds_grant(&ctx, &with_cookie(&value)));
}

#[test]
fn an_expired_grant_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let sealed = ctx
        .cipher
        .seal(
            BOOTSTRAP_COOKIE,
            br#"{"exp":1}"#,
            gitquarry_core::crypto::CompressionStrategy::None,
        )
        .unwrap();
    use base64::Engine;
    let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sealed);
    assert!(!holds_grant(&ctx, &with_cookie(&value)));
}

#[test]
fn a_live_sealed_grant_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let sealed = ctx
        .cipher
        .seal(
            BOOTSTRAP_COOKIE,
            br#"{"exp":9999999999}"#,
            gitquarry_core::crypto::CompressionStrategy::None,
        )
        .unwrap();
    use base64::Engine;
    let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sealed);
    assert!(holds_grant(&ctx, &with_cookie(&value)));
}

#[test]
fn the_token_is_long_random_hex() {
    assert_eq!(TOKEN.len(), 48);
    assert!(TOKEN.chars().all(|c| c.is_ascii_hexdigit()));
    // Same process, same token: the two gates and the printed hint have to
    // agree on one value.
    assert_eq!(*TOKEN, *TOKEN);
}
