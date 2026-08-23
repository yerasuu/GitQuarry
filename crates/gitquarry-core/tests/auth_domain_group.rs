use gitquarry_core::auth::domain::group::*;
use gitquarry_core::auth::entities::{Group, Permission, Team, User};
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

/// A stored user `set_member`/`remove_member` can resolve by name — those
/// look the acting username up for real (a typo must not silently grant
/// nobody), so a test needs an actual record in the bucket, not just a uid.
async fn stored_user(ctx: &gitquarry_core::app::AppContext, username: &str) -> User {
    let user = User {
        uid: Uid::new(),
        username: username.to_string(),
        ..Default::default()
    };
    gitquarry_core::auth::access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, None)
        .await
        .unwrap();
    user
}

/// Regression: without this, a freshly created group came out with no
/// members at all — manageable only through the global-admin bypass
/// `access::can_manage_group` also grants, which strands the group the
/// moment its creator is demoted (nobody left who can add a member, since
/// adding one already needs to manage the group).
#[tokio::test]
async fn create_makes_the_creator_the_sole_owner_member() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let creator = Uid::new();
    create(&ctx, "acme", None, creator.clone()).await.unwrap().unwrap();
    let (group, _etag) = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap();
    let group = group.unwrap();
    assert_eq!(
        group.members,
        std::collections::BTreeMap::from([(creator, Permission::Owner)])
    );
}

/// `set_member` is also the change-role action: a second call for the same
/// user overwrites the tier rather than adding a second entry.
#[tokio::test]
async fn set_member_adds_then_changes_a_role() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let alice = stored_user(&ctx, "alice").await;
    let mut group = Group::default();
    set_member(&ctx, &mut group, "alice", "developer", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(group.role_of(&alice.uid), Some(Permission::Developer));
    set_member(&ctx, &mut group, "alice", "maintainer", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(group.members.len(), 1);
    assert_eq!(group.role_of(&alice.uid), Some(Permission::Maintainer));
}

#[tokio::test]
async fn set_member_rejects_an_unknown_tier_and_an_unknown_user() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    stored_user(&ctx, "alice").await;
    let mut group = Group::default();
    assert!(
        set_member(&ctx, &mut group, "alice", "archduke", gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
    assert!(
        set_member(&ctx, &mut group, "nobody", "developer", gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
    assert!(group.members.is_empty());
}

/// Regression: the legacy fields still grant access until
/// `migrate-group-members` runs, so dropping only the `members` entry would
/// leave a removed member with their old tier fully intact — a silent no-op
/// exactly where one is least acceptable.
#[tokio::test]
async fn remove_member_also_strips_the_legacy_grants() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let alice = stored_user(&ctx, "alice").await;
    let mut group = Group {
        members: std::collections::BTreeMap::from([(alice.uid.clone(), Permission::Developer)]),
        legacy_owners: std::collections::BTreeSet::from([alice.uid.clone()]),
        legacy_teams: vec![Team {
            name: "everyone".to_string(),
            members: std::collections::BTreeSet::from([alice.uid.clone()]),
            permission: Permission::Maintainer,
            repos: None,
        }],
        ..Default::default()
    };
    remove_member(&ctx, &mut group, "alice").await.unwrap();
    assert_eq!(group.role_of(&alice.uid), None);
}
