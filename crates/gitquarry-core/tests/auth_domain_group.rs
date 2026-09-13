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
        federation: None,
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
    create(&ctx, "acme", None, creator.clone(), None, gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
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
/// the `group-members` data migration runs, so dropping only the `members` entry would
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

/// A subgroup's stored `name` is its full path — that's what makes it a repo
/// namespace and what `access::ancestor_group_names` reads the inheritance
/// chain off of.
#[tokio::test]
async fn create_nests_a_subgroup_under_its_parent_path() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let creator = Uid::new();
    create(&ctx, "acme", None, creator.clone(), None, gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    let parent = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    create(&ctx, "team", None, creator, Some(&parent), gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    let child = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme/team")
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(child.name, "acme/team");
    assert_eq!(child.parent, Some(parent.uid.clone()));
    // The child index is the only way to enumerate subgroups without
    // listing every group in the bucket.
    let children = gitquarry_core::auth::access::list_child_groups(ctx.backend().as_ref(), &ctx.cipher, &parent.uid)
        .await
        .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "acme/team");
}

/// A group path and a repo path share one URL space, so `/acme/team` can't
/// mean both a subgroup and a repo.
#[tokio::test]
async fn create_refuses_a_path_an_existing_repo_already_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    gitquarry_core::storage::settings::claim_repo_name(&backend, &cipher, "acme/team", &Uid::new(), None)
        .await
        .unwrap();
    let ctx = test_ctx(backend, cipher);
    let creator = Uid::new();
    create(&ctx, "acme", None, creator.clone(), None, gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    let parent = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    assert!(
        create(&ctx, "team", None, creator, Some(&parent), gitquarry_i18n::Locale::En)
            .await
            .unwrap()
            .is_none()
    );
}

/// Deleting a parent while its subgroups still exist would free a namespace
/// whose descendants' names are paths under it — someone else could then
/// claim it and inherit membership over repos nobody meant to hand them.
#[tokio::test]
async fn delete_refuses_while_subgroups_remain() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let creator = Uid::new();
    create(&ctx, "acme", None, creator.clone(), None, gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    let parent = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    create(&ctx, "team", None, creator, Some(&parent), gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    assert!(delete(&ctx, "acme", gitquarry_i18n::Locale::En).await.is_err());
    delete(&ctx, "acme/team", gitquarry_i18n::Locale::En).await.unwrap();
    delete(&ctx, "acme", gitquarry_i18n::Locale::En).await.unwrap();
    assert!(
        gitquarry_core::auth::access::list_child_groups(ctx.backend().as_ref(), &ctx.cipher, &parent.uid)
            .await
            .unwrap()
            .is_empty()
    );
}

/// The other half of the same rule: a repo can't take a path a group
/// namespace already holds, or `/acme/team` would answer to both.
#[tokio::test]
async fn create_repo_refuses_a_path_a_subgroup_already_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let creator = Uid::new();
    create(&ctx, "acme", None, creator.clone(), None, gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    let parent = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    create(&ctx, "team", None, creator, Some(&parent), gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    assert!(ctx.create_repo("acme/team").await.unwrap().is_none());
    // A path the subgroup doesn't hold is unaffected.
    assert!(ctx.create_repo("acme/team/web").await.unwrap().is_some());
}

/// "The groups I belong to" has to come off an index, not a scan of every
/// group in the bucket — and a stale pointer (a lost CAS, a membership
/// removed elsewhere) must be dropped on read rather than trusted.
#[tokio::test]
async fn list_groups_for_user_reads_the_index_and_drops_stale_pointers() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let ctx = test_ctx(backend, cipher);
    let alice = stored_user(&ctx, "alice").await;
    let creator = Uid::new();
    create(&ctx, "acme", None, creator.clone(), None, gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
    let (group, etag) = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap();
    let mut group = group.unwrap();
    set_member(&ctx, &mut group, "alice", "developer", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    gitquarry_core::auth::access::put_group(ctx.backend().as_ref(), &ctx.cipher, &group, etag.as_deref())
        .await
        .unwrap();
    let mine = gitquarry_core::auth::access::list_groups_for_user(ctx.backend().as_ref(), &ctx.cipher, &alice.uid)
        .await
        .unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].name, "acme");

    // The creator's own pointer resolves too — `create` indexes every
    // member it starts with, not just the ones added later.
    let creators = gitquarry_core::auth::access::list_groups_for_user(ctx.backend().as_ref(), &ctx.cipher, &creator)
        .await
        .unwrap();
    assert_eq!(creators.len(), 1);

    // A pointer whose group no longer lists this user is stale, not a
    // membership: dropped on read.
    let (group, etag) = gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap();
    let mut group = group.unwrap();
    group.members.remove(&alice.uid);
    gitquarry_core::auth::access::put_group(ctx.backend().as_ref(), &ctx.cipher, &group, etag.as_deref())
        .await
        .unwrap();
    assert!(
        gitquarry_core::auth::access::list_groups_for_user(ctx.backend().as_ref(), &ctx.cipher, &alice.uid)
            .await
            .unwrap()
            .is_empty()
    );
}
