//! `domain::release::add_asset`'s signature-verification path — checked
//! once at upload time against the uploader's own stored `gpg_keys`, see
//! `storage::remote::AssetMeta::signature`'s own doc comment. Skipped (not
//! failed) if `gpg` isn't on `PATH`, same tolerance the standalone `gpg`
//! module tests already have.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::domain::user;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::uid::Uid;
use gitquarry_domain::release::{NewAsset, add_asset, create};
use gitquarry_i18n::Locale;
use std::sync::Arc;

fn test_resolved(root: &std::path::Path) -> ResolvedRepo {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[37u8; SALT_LEN]).unwrap());
    let remote = Remote::new(backend, "repos/team/proj".to_string(), cipher);
    ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
    }
}

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[37u8; SALT_LEN]).unwrap());
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
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn put_tag(resolved: &ResolvedRepo, tag: &str) {
    let mut manifest = Manifest::default();
    manifest.refs.insert(format!("refs/tags/{tag}"), "a".repeat(40));
    resolved.remote.put_manifest(&manifest, None).await.unwrap();
}

fn gpg_available() -> bool {
    std::process::Command::new("gpg").arg("--version").output().is_ok()
}

struct TestKey {
    homedir: tempfile::TempDir,
    armored_public: String,
}

fn generate_key() -> TestKey {
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
    let armored_public = String::from_utf8(out.stdout).unwrap();
    TestKey {
        homedir,
        armored_public,
    }
}

fn sign(key: &TestKey, data: &[u8]) -> String {
    let data_path = key.homedir.path().join("to_sign");
    std::fs::write(&data_path, data).unwrap();
    let out = std::process::Command::new("gpg")
        .arg("--homedir")
        .arg(key.homedir.path())
        .args(["--batch", "--yes", "--armor", "--detach-sign", "--output", "-"])
        .arg(&data_path)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
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
    .unwrap();
}

#[tokio::test]
async fn unsigned_asset_is_never_marked_verified() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    put_tag(&resolved, "v1.0").await;
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        "alice",
        Locale::En,
    )
    .await
    .unwrap();
    let src_dir = tempfile::tempdir().unwrap();
    let asset_path = src_dir.path().join("a.bin");
    tokio::fs::write(&asset_path, b"hello").await.unwrap();

    add_asset(
        &ctx,
        &resolved,
        NewAsset {
            tag: "v1.0",
            filename: "a.bin",
            source: &asset_path,
            size: 5,
            sha256: "deadbeef",
            uploaded_by: "alice",
            signature: None,
            locale: Locale::En,
        },
    )
    .await
    .unwrap();

    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert!(release.assets[0].signature.is_none());
    assert!(!release.assets[0].signature_verified);
}

#[tokio::test]
async fn signed_asset_verifies_against_the_uploaders_own_stored_key() {
    if !gpg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    let key = generate_key();
    user::add_gpg_key(&ctx, "alice", &key.armored_public, Locale::En)
        .await
        .unwrap();

    put_tag(&resolved, "v1.0").await;
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        "alice",
        Locale::En,
    )
    .await
    .unwrap();
    let src_dir = tempfile::tempdir().unwrap();
    let asset_path = src_dir.path().join("a.bin");
    let data = b"hello release asset";
    tokio::fs::write(&asset_path, data).await.unwrap();
    let signature = sign(&key, data);

    add_asset(
        &ctx,
        &resolved,
        NewAsset {
            tag: "v1.0",
            filename: "a.bin",
            source: &asset_path,
            size: data.len() as u64,
            sha256: "deadbeef",
            uploaded_by: "alice",
            signature: Some(&signature),
            locale: Locale::En,
        },
    )
    .await
    .unwrap();

    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert!(release.assets[0].signature.is_some());
    assert!(release.assets[0].signature_verified, "{:?}", release.assets[0]);
}

#[tokio::test]
async fn signed_asset_by_an_unrelated_key_is_not_verified() {
    if !gpg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    // Alice only has `other`'s key on file — not the one that actually
    // signed the asset below.
    let signer = generate_key();
    let other = generate_key();
    user::add_gpg_key(&ctx, "alice", &other.armored_public, Locale::En)
        .await
        .unwrap();

    put_tag(&resolved, "v1.0").await;
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        "alice",
        Locale::En,
    )
    .await
    .unwrap();
    let src_dir = tempfile::tempdir().unwrap();
    let asset_path = src_dir.path().join("a.bin");
    let data = b"hello release asset";
    tokio::fs::write(&asset_path, data).await.unwrap();
    let signature = sign(&signer, data);

    add_asset(
        &ctx,
        &resolved,
        NewAsset {
            tag: "v1.0",
            filename: "a.bin",
            source: &asset_path,
            size: data.len() as u64,
            sha256: "deadbeef",
            uploaded_by: "alice",
            signature: Some(&signature),
            locale: Locale::En,
        },
    )
    .await
    .unwrap();

    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert!(release.assets[0].signature.is_some());
    assert!(!release.assets[0].signature_verified);
}
