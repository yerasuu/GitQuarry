//! `domain::repo::set_required_reviewers`.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::domain::repo;
use gitquarry_core::storage::Backend;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn Backend> = Arc::new(gitquarry_core::storage::local::LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[31u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
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
        visible_repo_uids_cache: Default::default(),
    }
}

#[tokio::test]
async fn set_required_reviewers_rejects_an_unknown_username() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = repo::create(
        &ctx,
        "team/proj",
        None,
        None,
        gitquarry_core::storage::entities::Visibility::Public,
    )
    .await
    .unwrap()
    .unwrap();

    assert!(
        repo::set_required_reviewers(&ctx, &resolved, "nobody", gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
    let (config, _etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(config.required_reviewers.is_empty());
}

#[tokio::test]
async fn set_required_reviewers_stores_every_real_username() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = repo::create(
        &ctx,
        "team/proj",
        None,
        None,
        gitquarry_core::storage::entities::Visibility::Public,
    )
    .await
    .unwrap()
    .unwrap();
    for username in ["alice", "bob"] {
        user::create(
            &ctx,
            user::NewUser {
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
        .unwrap();
    }

    let (cas, reviewers) = repo::set_required_reviewers(&ctx, &resolved, "alice, bob", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(cas, gitquarry_core::storage::Cas::Ok);
    assert_eq!(reviewers.len(), 2);
    let (config, _etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(config.required_reviewers.contains("alice"));
    assert!(config.required_reviewers.contains("bob"));
}
