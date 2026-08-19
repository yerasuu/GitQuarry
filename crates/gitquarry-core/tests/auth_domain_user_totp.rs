//! `auth::domain::user`'s TOTP lifecycle: enroll/confirm/disable plus the
//! recovery-code side of it (mint at confirm, one-time consume, regenerate).

use gitquarry_auth_crypto::totp;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::user;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_i18n::Locale;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[11u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
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
        tracker: None,
        allow_push_create: false,
        events: Default::default(),
    }
}

async fn make_user(ctx: &AppContext, username: &str) {
    user::create(
        ctx,
        user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

async fn current_secret(ctx: &AppContext, username: &str) -> String {
    access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap()
        .0
        .unwrap()
        .totp_secret
        .unwrap()
}

#[tokio::test]
async fn confirm_totp_rejects_a_wrong_code_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    user::enable_totp(&ctx, "alice", Locale::En).await.unwrap();

    assert!(user::confirm_totp(&ctx, "alice", "000000", Locale::En).await.is_err());
    let (u, _) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let u = u.unwrap();
    assert!(!u.totp_enabled);
    assert!(u.totp_recovery_codes.is_empty());
}

#[tokio::test]
async fn confirm_totp_mints_recovery_codes_only_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    user::enable_totp(&ctx, "alice", Locale::En).await.unwrap();
    let secret = current_secret(&ctx, "alice").await;
    let good_code = live_code(&secret);
    let (_cas, plaintext) = user::confirm_totp(&ctx, "alice", &good_code, Locale::En).await.unwrap();
    assert_eq!(plaintext.len(), 10);

    let (u, _) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let u = u.unwrap();
    assert!(u.totp_enabled);
    assert_eq!(u.totp_recovery_codes.len(), 10);
    // Stored hashed, never as the plaintext handed back to the caller.
    for stored in &u.totp_recovery_codes {
        assert!(!plaintext.contains(stored));
    }
}

#[tokio::test]
async fn consume_recovery_code_is_one_time_use() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    user::enable_totp(&ctx, "alice", Locale::En).await.unwrap();
    let secret = current_secret(&ctx, "alice").await;
    let (_cas, codes) = user::confirm_totp(&ctx, "alice", &live_code(&secret), Locale::En)
        .await
        .unwrap();

    assert!(user::consume_recovery_code(&ctx, "alice", &codes[0]).await.unwrap());
    // Same code again: already spent.
    assert!(!user::consume_recovery_code(&ctx, "alice", &codes[0]).await.unwrap());
    // A different, never-used code from the same batch still works.
    assert!(user::consume_recovery_code(&ctx, "alice", &codes[1]).await.unwrap());

    let (u, _) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    assert_eq!(u.unwrap().totp_recovery_codes.len(), 8);
}

#[tokio::test]
async fn consume_recovery_code_rejects_garbage_and_disabled_2fa() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    // 2FA never even enabled.
    assert!(!user::consume_recovery_code(&ctx, "alice", "whatever").await.unwrap());

    user::enable_totp(&ctx, "alice", Locale::En).await.unwrap();
    let secret = current_secret(&ctx, "alice").await;
    user::confirm_totp(&ctx, "alice", &live_code(&secret), Locale::En)
        .await
        .unwrap();
    assert!(
        !user::consume_recovery_code(&ctx, "alice", "not-a-real-code")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn regenerate_recovery_codes_invalidates_the_old_batch() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    user::enable_totp(&ctx, "alice", Locale::En).await.unwrap();
    let secret = current_secret(&ctx, "alice").await;
    let (_cas, old_codes) = user::confirm_totp(&ctx, "alice", &live_code(&secret), Locale::En)
        .await
        .unwrap();

    let (_cas, new_codes) = user::regenerate_recovery_codes(&ctx, "alice", Locale::En)
        .await
        .unwrap();
    assert_eq!(new_codes.len(), 10);
    assert_ne!(old_codes, new_codes);

    // Old batch is dead.
    assert!(!user::consume_recovery_code(&ctx, "alice", &old_codes[0]).await.unwrap());
    // New batch works.
    assert!(user::consume_recovery_code(&ctx, "alice", &new_codes[0]).await.unwrap());
}

#[tokio::test]
async fn regenerate_recovery_codes_requires_2fa_already_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    assert!(
        user::regenerate_recovery_codes(&ctx, "alice", Locale::En)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn disable_totp_clears_secret_and_every_recovery_code() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    user::enable_totp(&ctx, "alice", Locale::En).await.unwrap();
    let secret = current_secret(&ctx, "alice").await;
    user::confirm_totp(&ctx, "alice", &live_code(&secret), Locale::En)
        .await
        .unwrap();

    user::disable_totp(&ctx, "alice", Locale::En).await.unwrap();
    let (u, _) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let u = u.unwrap();
    assert!(!u.totp_enabled);
    assert!(u.totp_secret.is_none());
    assert!(u.totp_recovery_codes.is_empty());
}

fn live_code(secret_b32: &str) -> String {
    let raw = data_encoding::BASE32_NOPAD
        .decode(secret_b32.trim_end_matches('=').as_bytes())
        .unwrap();
    let counter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / totp::STEP_SECS;
    format!("{:06}", totp::hotp(&raw, counter).unwrap())
}
