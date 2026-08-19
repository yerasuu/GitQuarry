//! `domain::repo::delete`/`restore`/`purge` and the grace-window sweep
//! (`mirror::deleted_repo_sweep`) that ties them together.

use gitquarry_core::app::AppContext;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::domain::repo;
use gitquarry_core::mirror::deleted_repo_sweep;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::{self, Backend};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn Backend> = Arc::new(gitquarry_core::storage::local::LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[7u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
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

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

#[tokio::test]
async fn delete_hides_the_repo_then_restore_undoes_it() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = repo::create(&ctx, "team/proj", None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    repo::delete(&ctx, &resolved, "team/proj").await.unwrap();
    let (config, _etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(config.deleted_at.is_some());
    assert!(
        !storage::settings::list_visible_repo_uids(ctx.backend().as_ref())
            .await
            .unwrap()
            .contains(&resolved.uid)
    );
    assert!(
        storage::settings::list_deleted_repo_uids(ctx.backend().as_ref())
            .await
            .unwrap()
            .contains(&resolved.uid)
    );

    repo::restore(&ctx, &resolved).await.unwrap();
    let (config, _etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(config.deleted_at.is_none());
    assert!(
        storage::settings::list_visible_repo_uids(ctx.backend().as_ref())
            .await
            .unwrap()
            .contains(&resolved.uid)
    );
    assert!(
        !storage::settings::list_deleted_repo_uids(ctx.backend().as_ref())
            .await
            .unwrap()
            .contains(&resolved.uid)
    );
}

#[tokio::test]
async fn delete_twice_and_restore_without_deleting_both_reject() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = repo::create(&ctx, "team/proj", None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    assert!(repo::restore(&ctx, &resolved).await.is_err());
    repo::delete(&ctx, &resolved, "team/proj").await.unwrap();
    assert!(repo::delete(&ctx, &resolved, "team/proj").await.is_err());
}

#[tokio::test]
async fn purge_wipes_the_repo_and_frees_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = repo::create(&ctx, "team/proj", None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    repo::delete(&ctx, &resolved, "team/proj").await.unwrap();
    repo::purge(&ctx, &resolved, "team/proj").await.unwrap();

    assert!(!resolved.remote.exists().await.unwrap());
    assert!(ctx.resolve_repo("team/proj").await.unwrap().is_none());
    // The name claim was released -- a brand-new repo can take it again.
    assert!(
        repo::create(&ctx, "team/proj", None, None, Visibility::Public)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn purge_on_a_live_repo_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = repo::create(&ctx, "team/proj", None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    // Force-purge (the admin `purge_repo` action) only ever skips the rest
    // of an already-running grace window -- it must never be a shortcut
    // past `delete` itself.
    assert!(repo::purge(&ctx, &resolved, "team/proj").await.is_err());
    assert!(resolved.remote.exists().await.unwrap());
}

#[tokio::test]
async fn sweep_purges_only_repos_past_their_grace_window() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());

    let expired = repo::create(&ctx, "team/expired", None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    repo::delete(&ctx, &expired, "team/expired").await.unwrap();
    // Backdate past the grace window directly through storage -- `delete`
    // itself always stamps "now", there's no public API for a fake past
    // deletion.
    let (current, etag) = expired.remote.get_repo_config().await.unwrap();
    let backdated = gitquarry_core::storage::entities::RepoConfig {
        deleted_at: Some(now_secs() - repo::DELETE_GRACE_SECS - 100),
        ..current
    };
    storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        expired.uid.as_str(),
        &backdated,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let fresh = repo::create(&ctx, "team/fresh", None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    repo::delete(&ctx, &fresh, "team/fresh").await.unwrap();

    deleted_repo_sweep::sweep_once(&ctx).await.unwrap();

    assert!(ctx.resolve_repo("team/expired").await.unwrap().is_none());
    let still_there = ctx.resolve_repo("team/fresh").await.unwrap().unwrap();
    let (config, _etag) = still_there.remote.get_repo_config().await.unwrap();
    assert!(config.deleted_at.is_some());
}

#[test]
fn grace_seconds_remaining_hits_zero_once_expired() {
    assert!(repo::grace_seconds_remaining(now_secs()) > repo::DELETE_GRACE_SECS - 5);
    assert_eq!(
        repo::grace_seconds_remaining(now_secs() - repo::DELETE_GRACE_SECS - 100),
        0
    );
}
