//! `auth::access::check_quota` — the per-owner storage quota gate.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access::{self, put_group};
use gitquarry_core::auth::domain;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::entities::{OwnerRef, RepoConfig};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::settings::{add_owner_index, put_repo_config};
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[27u8; SALT_LEN]).unwrap());
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

async fn seed_owned_repo(ctx: &AppContext, owner: &OwnerRef, pack_bytes: u64, lfs_bytes: u64) {
    let uid = Uid::new();
    put_repo_config(
        ctx.backend().as_ref(),
        uid.as_str(),
        &RepoConfig {
            uid: uid.clone(),
            owner: Some(owner.clone()),
            pack_bytes,
            lfs_bytes,
            ..Default::default()
        },
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();
    add_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner, &uid).await;
}

async fn make_user(ctx: &AppContext, username: &str, quota_bytes: Option<u64>) -> OwnerRef {
    domain::user::create(
        ctx,
        domain::user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
    domain::user::set_storage_quota(ctx, username, quota_bytes, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let (user, _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap();
    OwnerRef::User(user.unwrap().uid)
}

async fn make_group(ctx: &AppContext, name: &str, quota_bytes: Option<u64>) -> OwnerRef {
    domain::group::create(ctx, name, None, Uid::new(), None, gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    let (group, etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, name)
        .await
        .unwrap();
    let mut group = group.unwrap();
    domain::group::set_storage_quota(&mut group, quota_bytes);
    put_group(ctx.backend().as_ref(), &ctx.cipher, &group, etag.as_deref())
        .await
        .unwrap();
    OwnerRef::Group(group.uid)
}

#[tokio::test]
async fn no_owner_never_triggers_a_quota_check() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    assert!(
        access::check_quota(ctx.backend().as_ref(), &ctx.cipher, None)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn user_with_no_quota_set_is_never_over() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner = make_user(&ctx, "alice", None).await;

    seed_owned_repo(&ctx, &owner, u64::MAX / 2, u64::MAX / 2).await;
    assert!(
        access::check_quota(ctx.backend().as_ref(), &ctx.cipher, Some(&owner))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn user_under_their_quota_is_fine_over_it_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner = make_user(&ctx, "alice", Some(1_000)).await;

    seed_owned_repo(&ctx, &owner, 400, 100).await;
    assert!(
        access::check_quota(ctx.backend().as_ref(), &ctx.cipher, Some(&owner))
            .await
            .unwrap()
            .is_none()
    );

    seed_owned_repo(&ctx, &owner, 600, 0).await;
    let over = access::check_quota(ctx.backend().as_ref(), &ctx.cipher, Some(&owner))
        .await
        .unwrap();
    assert_eq!(over, Some((1_100, 1_000)));
}

#[tokio::test]
async fn group_owner_is_checked_against_its_own_quota() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner = make_group(&ctx, "acme", Some(500)).await;

    seed_owned_repo(&ctx, &owner, 500, 0).await;
    let over = access::check_quota(ctx.backend().as_ref(), &ctx.cipher, Some(&owner))
        .await
        .unwrap();
    assert_eq!(over, Some((500, 500)));
}

#[tokio::test]
async fn a_stale_owner_that_no_longer_resolves_is_treated_as_unlimited() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    // Never created -- `RepoConfig::owner` pointing at a since-deleted user.
    let owner = OwnerRef::User(Uid::new());
    seed_owned_repo(&ctx, &owner, u64::MAX / 2, u64::MAX / 2).await;
    assert!(
        access::check_quota(ctx.backend().as_ref(), &ctx.cipher, Some(&owner))
            .await
            .unwrap()
            .is_none()
    );
}
