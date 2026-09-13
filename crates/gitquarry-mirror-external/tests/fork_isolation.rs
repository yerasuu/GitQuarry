//! A duplicated repo must not inherit its source's external mirrors — the
//! only place in a repo's own storage that holds a live, reusable
//! third-party credential. Lives here rather than in `gitquarry-core`'s own
//! tests because writing an `ExternalMirror` needs this crate.

use gitquarry_core::app::{AppContext, Locks};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::uid::Uid;
use gitquarry_mirror_external::{access, domain};
use std::sync::Arc;
use std::time::Duration;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend: Arc::new(backend) as Arc<dyn Backend>,
        locks: Locks::default(),
        cipher,
        mirror_max_age: Duration::from_secs(60),
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

#[tokio::test]
async fn a_fork_doesnt_inherit_the_sources_mirror_credential() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let source = gitquarry_core::domain::repo::create(&ctx, "alice/mirrored", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://github.com/alice/mirrored.git".to_string(),
            token: "ghp_alices_secret".to_string(),
            enabled: true,
            label: "github".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let fork = gitquarry_core::domain::repo::fork(
        &ctx,
        &source,
        "bob/mirrored",
        None,
        OwnerRef::User(Uid::from("u-bob".to_string())),
        gitquarry_i18n::Locale::DEFAULT,
    )
    .await
    .unwrap()
    .unwrap();

    let inherited = domain::list(&ctx, &fork.uid).await.unwrap();
    assert!(
        inherited.is_empty(),
        "the fork inherited {} mirror(s), credential included",
        inherited.len()
    );
    assert_eq!(domain::list(&ctx, &source.uid).await.unwrap().len(), 1);
}
