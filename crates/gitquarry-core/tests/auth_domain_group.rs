use gitquarry_core::auth::domain::group::*;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::uid::Uid;

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = Cipher::derive("test passphrase", &[6u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

fn test_ctx(backend: LocalBackend, cipher: Cipher) -> gitquarry_core::app::AppContext {
    gitquarry_core::app::AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: std::path::PathBuf::new(),
        config_path: std::path::PathBuf::new(),
        exe_path: std::path::PathBuf::new(),
        backend: std::sync::Arc::new(backend),
        locks: Default::default(),
        cipher: std::sync::Arc::new(cipher),
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

/// Regression: without this, a freshly created group came out with an
/// empty `owners` set — manageable only through the global-admin bypass
/// `access::can_manage_group` also grants, which strands the group the
/// moment its creator is demoted (nobody left who can add an owner, since
/// adding one already needs to manage the group).
#[tokio::test]
async fn create_makes_the_creator_the_sole_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let creator = Uid::new();
    create(&ctx, "acme", None, creator.clone()).await.unwrap().unwrap();
    let (group, _etag) = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap();
    let group = group.unwrap();
    assert_eq!(group.owners, std::collections::BTreeSet::from([creator]));
}

#[tokio::test]
async fn parse_team_repos_empty_means_unscoped() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    assert_eq!(
        parse_team_repos(&ctx, "acme", "", gitquarry_i18n::Locale::Es)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        parse_team_repos(&ctx, "acme", "  ", gitquarry_i18n::Locale::Es)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn parse_team_repos_splits_on_comma_and_resolves_each_to_a_uid() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let web = Uid::new();
    let api = Uid::new();
    gitquarry_core::storage::settings::claim_repo_name(&backend, &cipher, "acme/web", &web, None)
        .await
        .unwrap();
    gitquarry_core::storage::settings::claim_repo_name(&backend, &cipher, "acme/api", &api, None)
        .await
        .unwrap();
    let ctx = test_ctx(backend, cipher);
    let repos = parse_team_repos(&ctx, "acme", "web, api ,  ", gitquarry_i18n::Locale::Es)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repos.len(), 2);
    assert!(repos.contains(&web));
    assert!(repos.contains(&api));
}

#[tokio::test]
async fn parse_team_repos_rejects_an_unknown_repo() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    assert!(
        parse_team_repos(&ctx, "acme", "nope", gitquarry_i18n::Locale::Es)
            .await
            .is_err()
    );
}
