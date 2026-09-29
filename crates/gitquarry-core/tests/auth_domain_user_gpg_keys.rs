//! `auth::domain::user::{add_gpg_key, remove_gpg_key}`. Skipped (not
//! failed) if `gpg` isn't on `PATH` — same tolerance the standalone `gpg`
//! module tests already have.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::user;
use gitquarry_i18n::Locale;

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
    .unwrap();
}

fn gpg_available() -> bool {
    std::process::Command::new("gpg").arg("--version").output().is_ok()
}

fn generate_key() -> String {
    let homedir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(homedir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let batch = "\
%no-protection
Key-Type: eddsa
Key-Curve: ed25519
Name-Real: Test User
Name-Email: test@example.com
Expire-Date: 0
%commit
";
    let batch_path = homedir.path().join("batch");
    std::fs::write(&batch_path, batch).unwrap();
    let status = std::process::Command::new("gpg")
        .arg("--homedir")
        .arg(homedir.path())
        .args(["--batch", "--yes", "--no-tty", "--gen-key"])
        .arg(&batch_path)
        .status()
        .unwrap();
    assert!(status.success());
    let out = std::process::Command::new("gpg")
        .arg("--homedir")
        .arg(homedir.path())
        .args(["--batch", "--armor", "--export", "test@example.com"])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
async fn add_gpg_key_rejects_garbage() {
    if !gpg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    make_user(&ctx, "alice").await;

    assert!(user::add_gpg_key(&ctx, "alice", "not a key", Locale::En).await.is_err());
    let (user, _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    assert!(user.unwrap().gpg_keys.is_empty());
}

#[tokio::test]
async fn add_gpg_key_then_remove_by_fingerprint_roundtrips() {
    if !gpg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    make_user(&ctx, "alice").await;
    let key = generate_key();

    user::add_gpg_key(&ctx, "alice", &key, Locale::En).await.unwrap();
    let (user, _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let user = user.unwrap();
    assert_eq!(user.gpg_keys.len(), 1);
    assert_eq!(user.gpg_keys[0].fingerprint.len(), 40);
    let fingerprint = user.gpg_keys[0].fingerprint.clone();

    user::remove_gpg_key(&ctx, "alice", &fingerprint, Locale::En)
        .await
        .unwrap();
    let (user, _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    assert!(user.unwrap().gpg_keys.is_empty());
}

#[tokio::test]
async fn remove_gpg_key_is_a_noop_for_an_unknown_fingerprint() {
    if !gpg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    make_user(&ctx, "alice").await;
    let key = generate_key();
    user::add_gpg_key(&ctx, "alice", &key, Locale::En).await.unwrap();

    user::remove_gpg_key(&ctx, "alice", "0000000000000000000000000000000000000000", Locale::En)
        .await
        .unwrap();
    let (user, _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    assert_eq!(user.unwrap().gpg_keys.len(), 1);
}
