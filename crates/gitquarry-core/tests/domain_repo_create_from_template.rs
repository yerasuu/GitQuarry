//! `domain::repo::create_from_template`.

use gitquarry_core::app::AppContext;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::domain::repo;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{self, Backend};
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn Backend> = Arc::new(gitquarry_core::storage::local::LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
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

#[tokio::test]
async fn rejects_a_source_repo_that_is_not_marked_as_a_template() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let source = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let result = repo::create_from_template(
        &ctx,
        &source,
        "team/copy",
        None,
        OwnerRef::User(Uid::from("team".to_string())),
        gitquarry_i18n::Locale::Es,
    )
    .await;
    match result {
        Ok(_) => panic!("expected an error for a non-template source"),
        Err(err) => assert!(err.to_string().contains("plantilla")),
    }
    assert!(
        storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, "team/copy")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn copies_config_and_content_from_a_template_source() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let source = repo::create(
        &ctx,
        "team/proj",
        None,
        None,
        Some("original description".to_string()),
        Visibility::Limited,
    )
    .await
    .unwrap()
    .unwrap();
    let (mut config, etag) = source.remote.get_repo_config().await.unwrap();
    config.is_template = true;
    storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        source.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let dest_owner = OwnerRef::User(Uid::from("alice".to_string()));
    let dest = repo::create_from_template(
        &ctx,
        &source,
        "alice/copy",
        None,
        dest_owner.clone(),
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap()
    .unwrap();

    let (dest_config, _etag) = dest.remote.get_repo_config().await.unwrap();
    assert_eq!(dest_config.description.as_deref(), Some("original description"));
    assert_eq!(dest_config.visibility, Visibility::Limited);
    assert_eq!(dest_config.owner, Some(dest_owner));
    // The copy isn't itself a template unless someone explicitly marks it so.
    assert!(!dest_config.is_template);
    assert!(dest_config.forked_from.is_none());
}

#[tokio::test]
async fn returns_none_when_the_destination_name_is_already_taken() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let source = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let (mut config, etag) = source.remote.get_repo_config().await.unwrap();
    config.is_template = true;
    storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        source.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();
    repo::create(&ctx, "team/copy", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let result = repo::create_from_template(
        &ctx,
        &source,
        "team/copy",
        None,
        OwnerRef::User(Uid::from("team".to_string())),
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    assert!(result.is_none());
}
