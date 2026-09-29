use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Group, Permission, Team, User};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::migrations;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::{backend, cipher};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

async fn store_user(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>, username: &str) -> User {
    let user = User {
        uid: Uid::new(),
        username: username.to_string(),
        ..Default::default()
    };
    access::put_user(backend.as_ref(), cipher, &user, None).await.unwrap();
    user.claim_uid(backend.as_ref(), cipher).await.unwrap();
    user
}

/// A group exactly as an older release stored it: membership in `owners` and
/// `teams`, nothing in `members`, no `user-group` index pointer.
async fn store_legacy_group(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    name: &str,
    owners: &[&User],
    teams: Vec<Team>,
) -> Group {
    let group = Group {
        uid: Uid::new(),
        name: name.to_string(),
        legacy_owners: owners.iter().map(|u| u.uid.clone()).collect(),
        legacy_teams: teams,
        ..Default::default()
    };
    access::put_group(backend.as_ref(), cipher, &group, None).await.unwrap();
    group.claim_uid(backend.as_ref(), cipher).await.unwrap();
    group
}

#[tokio::test]
async fn a_fresh_bucket_reports_version_zero_and_every_migration_pending() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    assert_eq!(
        migrations::stored_version(backend.as_ref(), &cipher)
            .await
            .unwrap()
            .version,
        0
    );
    assert_eq!(
        migrations::pending(backend.as_ref(), &cipher).await.unwrap().len(),
        migrations::MIGRATIONS.len()
    );
}

/// The dry run must not touch anything — it's what `gitquarry migrate`
/// prints before an operator decides to deploy.
#[tokio::test]
async fn plan_reports_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let alice = store_user(&backend, &cipher, "alice").await;
    store_legacy_group(&backend, &cipher, "acme", &[&alice], Vec::new()).await;

    let planned = migrations::plan(&backend, &cipher).await.unwrap();
    let group_members = planned.iter().find(|(m, _)| m.name == "group-members").unwrap();
    assert!(group_members.1.changed > 0);

    let stored = access::get_group(backend.as_ref(), &cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    assert!(stored.members.is_empty());
    assert_eq!(stored.legacy_owners.len(), 1);
    assert_eq!(
        migrations::stored_version(backend.as_ref(), &cipher)
            .await
            .unwrap()
            .version,
        0
    );
}

#[tokio::test]
async fn run_pending_folds_legacy_membership_and_stamps_the_version() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let alice = store_user(&backend, &cipher, "alice").await;
    let bob = store_user(&backend, &cipher, "bob").await;
    store_legacy_group(
        &backend,
        &cipher,
        "acme",
        &[&alice],
        vec![Team {
            name: "everyone".to_string(),
            members: BTreeSet::from([bob.uid.clone()]),
            permission: Permission::Developer,
            repos: None,
        }],
    )
    .await;

    let applied = migrations::run_pending(&backend, &cipher).await.unwrap();
    assert_eq!(applied.len(), migrations::MIGRATIONS.len());
    assert!(applied.iter().all(|(_, outcome)| outcome.incomplete == 0));

    let stored = access::get_group(backend.as_ref(), &cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(stored.members.get(&alice.uid), Some(&Permission::Owner));
    assert_eq!(stored.members.get(&bob.uid), Some(&Permission::Developer));
    assert!(stored.legacy_owners.is_empty());
    assert!(stored.legacy_teams.is_empty());

    // Both members are now findable without scanning every group.
    for uid in [&alice.uid, &bob.uid] {
        let mine = access::list_groups_for_user(backend.as_ref(), &cipher, uid)
            .await
            .unwrap();
        assert_eq!(mine.len(), 1, "{uid} should belong to one group");
    }
    assert_eq!(
        migrations::stored_version(backend.as_ref(), &cipher)
            .await
            .unwrap()
            .version,
        migrations::CURRENT_DATA_VERSION
    );
}

/// A team that only covered some of the group's repos must not become a
/// group-wide role — that would hand its members that tier on every other
/// repo in the group. It becomes an explicit per-repo grant instead.
#[tokio::test]
async fn a_scoped_team_becomes_a_per_repo_grant_not_a_group_role() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let alice = store_user(&backend, &cipher, "alice").await;
    let web = Uid::new();
    store_legacy_group(
        &backend,
        &cipher,
        "acme",
        &[],
        vec![Team {
            name: "frontend".to_string(),
            members: BTreeSet::from([alice.uid.clone()]),
            permission: Permission::Maintainer,
            repos: Some(BTreeSet::from([web.clone()])),
        }],
    )
    .await;

    migrations::run_pending(&backend, &cipher).await.unwrap();

    let group = access::get_group(backend.as_ref(), &cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    assert!(group.members.is_empty(), "a scoped team grants nothing group-wide");
    let stored = access::get_user(backend.as_ref(), &cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(stored.repos.get(&web), Some(&Permission::Maintainer));
}

/// An existing higher grant survives: the migration only ever raises, which
/// is also what makes re-running it safe.
#[tokio::test]
async fn a_scoped_team_never_lowers_a_grant_the_user_already_holds() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let web = Uid::new();
    let alice = User {
        uid: Uid::new(),
        username: "alice".to_string(),
        repos: BTreeMap::from([(web.clone(), Permission::Owner)]),
        ..Default::default()
    };
    access::put_user(backend.as_ref(), &cipher, &alice, None).await.unwrap();
    alice.claim_uid(backend.as_ref(), &cipher).await.unwrap();
    store_legacy_group(
        &backend,
        &cipher,
        "acme",
        &[],
        vec![Team {
            name: "readers".to_string(),
            members: BTreeSet::from([alice.uid.clone()]),
            permission: Permission::Reporter,
            repos: Some(BTreeSet::from([web.clone()])),
        }],
    )
    .await;

    migrations::run_pending(&backend, &cipher).await.unwrap();

    let stored = access::get_user(backend.as_ref(), &cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(stored.repos.get(&web), Some(&Permission::Owner));
}

/// Idempotence is the contract every migration here has to meet: the version
/// is stamped only after a pass finishes, so a crash or a second instance
/// re-runs it.
#[tokio::test]
async fn running_twice_changes_nothing_the_second_time() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let alice = store_user(&backend, &cipher, "alice").await;
    store_legacy_group(&backend, &cipher, "acme", &[&alice], Vec::new()).await;

    migrations::run_pending(&backend, &cipher).await.unwrap();
    let after_first = access::get_group(backend.as_ref(), &cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();

    assert!(
        migrations::run_pending(&backend, &cipher).await.unwrap().is_empty(),
        "a stamped bucket has nothing pending"
    );
    let after_second = access::get_group(backend.as_ref(), &cipher, "acme")
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(after_first, after_second);
}

/// The audit trail is what tells an operator which release touched the
/// bucket, so it has to name the migration, not just count it.
#[tokio::test]
async fn the_stamp_records_what_was_applied() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    migrations::run_pending(&backend, &cipher).await.unwrap();
    let state = migrations::stored_version(backend.as_ref(), &cipher).await.unwrap();
    assert_eq!(state.version, migrations::CURRENT_DATA_VERSION);
    assert_eq!(state.applied.len(), migrations::MIGRATIONS.len());
    assert_eq!(state.applied[0].name, "group-members");
    assert!(!state.applied[0].at.is_empty());
}

/// Every entry in the registry must have a runner arm behind it — a new
/// migration added to the list without one would otherwise stamp itself as
/// applied while doing nothing.
#[tokio::test]
async fn every_registered_migration_is_dispatchable() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let planned = migrations::plan(&backend, &cipher).await.unwrap();
    assert_eq!(planned.len(), migrations::MIGRATIONS.len());
    assert_eq!(
        migrations::MIGRATIONS.last().map(|m| m.version),
        Some(migrations::CURRENT_DATA_VERSION),
        "CURRENT_DATA_VERSION must match the highest registered migration"
    );
}

/// A grant held in `User::repos` with no repo permission index entry — what
/// the admin user page wrote before 0.5.2 — must end up listed on the repo.
#[tokio::test]
async fn an_unindexed_grant_is_indexed_and_a_second_run_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let repo_uid = Uid::new();
    let mut alice = store_user(&backend, &cipher, "alice").await;
    alice.repos.insert(repo_uid.clone(), Permission::Developer);
    let (_, etag) = access::get_user(backend.as_ref(), &cipher, "alice").await.unwrap();
    access::put_user(backend.as_ref(), &cipher, &alice, etag.as_deref())
        .await
        .unwrap();
    assert!(
        access::list_repo_permissions(&backend, &cipher, &repo_uid)
            .await
            .unwrap()
            .is_empty()
    );

    let planned = migrations::plan(&backend, &cipher).await.unwrap();
    let index = planned.iter().find(|(m, _)| m.name == "repo-permission-index").unwrap();
    assert_eq!(index.1.changed, 1);
    assert!(
        access::list_repo_permissions(&backend, &cipher, &repo_uid)
            .await
            .unwrap()
            .is_empty()
    );

    migrations::run_pending(&backend, &cipher).await.unwrap();
    let granted: Vec<String> = access::list_repo_permissions(&backend, &cipher, &repo_uid)
        .await
        .unwrap()
        .into_iter()
        .map(|(u, p)| format!("{}:{p:?}", u.username))
        .collect();
    assert_eq!(granted, ["alice:Developer"]);

    let rerun = gitquarry_core::migrations::repo_permission_index::run(&backend, &cipher, migrations::Mode::Apply)
        .await
        .unwrap();
    assert_eq!(rerun.changed, 0);
}
