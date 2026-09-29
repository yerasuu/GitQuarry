//! `domain::release::add_asset`'s signature-verification path — checked
//! once at upload time against the uploader's own stored `gpg_keys`, see
//! `storage::remote::AssetMeta::signature`'s own doc comment. Skipped (not
//! failed) if `gpg` isn't on `PATH`, same tolerance the standalone `gpg`
//! module tests already have.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::domain::user;
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::uid::Uid;
use gitquarry_domain::release::{NewAsset, add_asset, create};
use gitquarry_i18n::Locale;
use gitquarry_test_support::ctx;
use std::sync::Arc;

fn test_resolved(ctx: &AppContext) -> ResolvedRepo {
    let remote = Remote::new(ctx.backend(), "repos/team/proj".to_string(), Arc::clone(&ctx.cipher));
    ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
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

#[tokio::test]
async fn unsigned_asset_is_never_marked_verified() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let resolved = test_resolved(&ctx);
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
    let ctx = ctx(dir.path());
    let resolved = test_resolved(&ctx);
    gitquarry_test_support::user(&ctx, "alice", false).await;
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
    let ctx = ctx(dir.path());
    let resolved = test_resolved(&ctx);
    gitquarry_test_support::user(&ctx, "alice", false).await;
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
