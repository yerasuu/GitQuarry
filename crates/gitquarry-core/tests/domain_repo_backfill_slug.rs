//! `domain::repo::backfill_slug` — the lazy migration that gives a repo
//! predating `RepoConfig::display_name` (or created before `sanitize_path`
//! tightened its charset) a clean slug the first time a uid-based listing
//! loads it.

use gitquarry_core::app::AppContext;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::domain::repo;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::{self, Backend};
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn Backend> = Arc::new(gitquarry_core::storage::local::LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[9u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
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
        ci: None,
        visible_repo_uids_cache: Default::default(),
    }
}

#[tokio::test]
async fn backfills_display_name_and_slug_for_a_legacy_full_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    // Simulates a repo whose name predates `sanitize_path`'s charset
    // tightening — `domain::repo::create` itself never validates the
    // charset (that's the caller's job), so this is a legitimate way to get
    // a legacy-shaped `full_name` into storage for the test.
    let resolved = repo::create(&ctx, "team/legacy name", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let (config, etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(config.display_name.is_none());

    let updated = repo::backfill_slug(
        Arc::clone(&ctx.backend),
        Arc::clone(&ctx.cipher),
        &resolved.uid,
        config,
        etag,
    )
    .await;

    assert_eq!(updated.display_name.as_deref(), Some("legacy name"));
    assert_eq!(updated.full_name, "team/legacy-name");
    // The new slug actually resolves, and the old broken name no longer does.
    assert!(
        storage::settings::resolve_repo_name(ctx.backend.as_ref(), &ctx.cipher, "team/legacy-name")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        storage::settings::resolve_repo_name(ctx.backend.as_ref(), &ctx.cipher, "team/legacy name")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn is_a_no_op_once_already_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = repo::create(&ctx, "team/legacy name", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let (config, etag) = resolved.remote.get_repo_config().await.unwrap();
    let once = repo::backfill_slug(
        Arc::clone(&ctx.backend),
        Arc::clone(&ctx.cipher),
        &resolved.uid,
        config,
        etag,
    )
    .await;

    let (config, etag) = resolved.remote.get_repo_config().await.unwrap();
    assert_eq!(config, once);
    let twice = repo::backfill_slug(
        Arc::clone(&ctx.backend),
        Arc::clone(&ctx.cipher),
        &resolved.uid,
        config,
        etag,
    )
    .await;
    assert_eq!(twice, once);
}
