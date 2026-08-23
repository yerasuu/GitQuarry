use gitquarry_core::auth::access::permissions::*;
use gitquarry_core::auth::entities::{Group, Permission, Team, User};
use gitquarry_core::storage::uid::Uid;

/// `repos` labels double as deterministic test uids via `Uid`'s
/// `From<String>` — the same label passed to a test's `effective_permission`
/// call (wrapped the same way) matches the key stored here.
fn user_with_repos(username: &str, repos: &[(&str, Permission)]) -> User {
    User {
        uid: Uid::new(),
        username: username.to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: repos.iter().map(|(r, p)| (Uid::from(r.to_string()), *p)).collect(),
        default_permission: None,
        password_hash: None,
        is_admin: false,
        session_epoch: 0,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        totp_last_step: 0,
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    }
}

fn ruid(label: &str) -> Uid {
    Uid::from(label.to_string())
}

/// `members` below takes `&User`, not raw name strings, since
/// `Group::members`/`Team::members` are uid-keyed — a test needs an actual
/// `User` in hand to reference its uid. `repos` labels double as
/// deterministic test uids via [`ruid`], same as `user_with_repos` — pass
/// full repo-path labels (`"acme/web"`), not group-local ones: `Team::repos`
/// holds full repo uids, not names relative to the group's own namespace.
fn team(name: &str, members: &[&User], permission: Permission, repos: Option<&[&str]>) -> Team {
    Team {
        name: name.to_string(),
        members: members.iter().map(|u| u.uid.clone()).collect(),
        permission,
        repos: repos.map(|rs| rs.iter().map(|r| ruid(r)).collect()),
    }
}

fn group(name: &str, members: &[(&User, Permission)]) -> Group {
    Group {
        uid: Default::default(),
        name: name.to_string(),
        description: None,
        members: members.iter().map(|(u, p)| (u.uid.clone(), *p)).collect(),
        storage_quota_bytes: None,
        legacy_owners: Default::default(),
        legacy_teams: Vec::new(),
    }
}

/// A group whose stored record still predates `Group::members` — what a
/// bucket looks like before `migrate-group-members` runs. Every test using
/// this is asserting that the upgrade doesn't silently revoke access.
fn legacy_group(name: &str, owners: &[&User], teams: Vec<Team>) -> Group {
    Group {
        uid: Default::default(),
        name: name.to_string(),
        description: None,
        members: Default::default(),
        storage_quota_bytes: None,
        legacy_owners: owners.iter().map(|u| u.uid.clone()).collect(),
        legacy_teams: teams,
    }
}

#[test]
fn effective_permission_prefers_explicit_repo_grant_over_group() {
    let alice = user_with_repos("alice", &[("acme/proj", Permission::Developer)]);
    let acme = group("acme", &[(&alice, Permission::Reporter)]);
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/proj", &ruid("acme/proj")),
        Some(Permission::Developer)
    );
}

#[test]
fn effective_permission_own_namespace_gets_implicit_owner() {
    // No explicit grant, no group named "alice" — just a personal repo
    // under alice's own username, same as `/_new` creates by default.
    let alice = user_with_repos("alice", &[]);
    let bob = user_with_repos("bob", &[]);
    assert_eq!(
        effective_permission(&alice, None, "alice/proj", &ruid("alice/proj")),
        Some(Permission::Owner)
    );
    // Someone else's personal namespace grants nothing.
    assert_eq!(
        effective_permission(&bob, None, "alice/proj", &ruid("alice/proj")),
        None
    );
}

/// The whole point of the direct-membership model: one entry on the group,
/// no per-repo grant, and it covers every repo in the group.
#[test]
fn effective_permission_applies_a_group_role_to_every_repo_in_the_group() {
    let alice = user_with_repos("alice", &[]);
    let acme = group("acme", &[(&alice, Permission::Developer)]);
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/web", &ruid("acme/web")),
        Some(Permission::Developer)
    );
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/api", &ruid("acme/api")),
        Some(Permission::Developer)
    );
}

#[test]
fn effective_permission_grants_nothing_to_a_non_member() {
    let alice = user_with_repos("alice", &[]);
    let bob = user_with_repos("bob", &[]);
    let acme = group("acme", &[(&alice, Permission::Developer)]);
    assert_eq!(
        effective_permission(&bob, Some(&acme), "acme/proj", &ruid("acme/proj")),
        None
    );
}

/// Regression: a bucket upgraded before `migrate-group-members` runs still
/// has its membership in `owners`/`teams`, and those must keep granting —
/// reading only `members` there would revoke everyone at once.
#[test]
fn effective_permission_still_honors_a_legacy_owner() {
    let alice = user_with_repos("alice", &[]);
    let acme = legacy_group("acme", &[&alice], vec![]);
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/proj", &ruid("acme/proj")),
        Some(Permission::Owner)
    );
}

#[test]
fn effective_permission_still_honors_an_unscoped_legacy_team() {
    let alice = user_with_repos("alice", &[]);
    let acme = legacy_group(
        "acme",
        &[],
        vec![team("everyone", &[&alice], Permission::Developer, None)],
    );
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/proj", &ruid("acme/proj")),
        Some(Permission::Developer)
    );
}

/// A legacy team scoped to a subset of the group's repos must not read as a
/// group-wide role — folding it into one would silently escalate its members
/// on every other repo in the group.
#[test]
fn effective_permission_keeps_a_scoped_legacy_team_scoped() {
    let alice = user_with_repos("alice", &[]);
    let acme = legacy_group(
        "acme",
        &[],
        vec![team("frontend", &[&alice], Permission::Developer, Some(&["acme/web"]))],
    );
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/web", &ruid("acme/web")),
        Some(Permission::Developer)
    );
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/backend", &ruid("acme/backend")),
        None
    );
}

#[test]
fn effective_permission_picks_the_highest_of_the_role_and_the_legacy_grants() {
    let alice = user_with_repos("alice", &[]);
    let mut acme = group("acme", &[(&alice, Permission::Reporter)]);
    acme.legacy_teams = vec![team("writers", &[&alice], Permission::Developer, Some(&["acme/proj"]))];
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/proj", &ruid("acme/proj")),
        Some(Permission::Developer)
    );
    // Outside the scoped team's repo list, the group-wide role is all
    // that's left.
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/other", &ruid("acme/other")),
        Some(Permission::Reporter)
    );
}

#[test]
fn effective_permission_falls_back_to_default_permission_when_no_group_matches() {
    let mut alice = user_with_repos("alice", &[]);
    alice.default_permission = Some(Permission::Reporter);
    assert_eq!(
        effective_permission(&alice, None, "someone/proj", &ruid("someone/proj")),
        Some(Permission::Reporter)
    );
}

/// `Owner` in `members` is the only tier that runs the group — a
/// `Maintainer` manages each repo's settings but never the membership that
/// hands out those repos.
#[test]
fn can_manage_group_allows_admin_and_owner_members_only() {
    let mut root = user_with_repos("root", &[]);
    root.is_admin = true;
    let alice = user_with_repos("alice", &[]);
    let mia = user_with_repos("mia", &[]);
    let bob = user_with_repos("bob", &[]);
    let acme = group("acme", &[(&alice, Permission::Owner), (&mia, Permission::Maintainer)]);
    assert!(can_manage_group(Some(&root), Some(&acme))); // global admin
    assert!(can_manage_group(Some(&alice), Some(&acme))); // owner member
    assert!(!can_manage_group(Some(&mia), Some(&acme))); // maintainer member
    assert!(!can_manage_group(Some(&bob), Some(&acme))); // not a member
}

#[test]
fn can_manage_group_still_honors_a_legacy_owner() {
    let alice = user_with_repos("alice", &[]);
    let acme = legacy_group("acme", &[&alice], vec![]);
    assert!(can_manage_group(Some(&alice), Some(&acme)));
}

/// `effective_members` is what the group page renders and what
/// `migrate-group-members` writes back: the union, at the highest tier each
/// user reaches, minus anyone whose only grant is a scoped legacy team
/// (they hold no group-wide tier at all).
#[test]
fn effective_members_folds_the_legacy_fields_at_the_highest_tier() {
    let alice = user_with_repos("alice", &[]);
    let mia = user_with_repos("mia", &[]);
    let scoped_only = user_with_repos("scoped", &[]);
    let mut acme = group("acme", &[(&alice, Permission::Reporter)]);
    acme.legacy_owners = std::collections::BTreeSet::from([mia.uid.clone()]);
    acme.legacy_teams = vec![
        team("everyone", &[&alice], Permission::Developer, None),
        team("frontend", &[&scoped_only], Permission::Maintainer, Some(&["acme/web"])),
    ];
    let members = acme.effective_members();
    assert_eq!(members.get(&alice.uid), Some(&Permission::Developer));
    assert_eq!(members.get(&mia.uid), Some(&Permission::Owner));
    assert_eq!(members.get(&scoped_only.uid), None);
}

#[test]
fn permission_tiers_order_low_to_high() {
    assert!(Permission::Guest < Permission::Reporter);
    assert!(Permission::Reporter < Permission::Developer);
    assert!(Permission::Developer < Permission::Maintainer);
    assert!(Permission::Maintainer < Permission::Owner);
}

/// Live `.conf/users/*.json`/`.conf/groups/*.json` data predates this
/// ladder and still stores the old `Permission::{Read,Write}` names —
/// the `#[serde(alias = ...)]` on `Reporter`/`Developer` must keep those
/// deserializing to the equivalent new tier, not just accept the new
/// tier names going forward.
#[test]
fn old_read_write_json_strings_still_deserialize() {
    assert_eq!(
        serde_json::from_str::<Permission>("\"read\"").unwrap(),
        Permission::Reporter
    );
    assert_eq!(
        serde_json::from_str::<Permission>("\"write\"").unwrap(),
        Permission::Developer
    );
    assert_eq!(
        serde_json::from_str::<Permission>("\"reporter\"").unwrap(),
        Permission::Reporter
    );
    assert_eq!(
        serde_json::from_str::<Permission>("\"owner\"").unwrap(),
        Permission::Owner
    );
}

#[test]
fn can_manage_repo_settings_needs_maintainer_or_admin() {
    assert!(can_manage_repo_settings(Some(Permission::Maintainer), false));
    assert!(can_manage_repo_settings(Some(Permission::Owner), false));
    assert!(!can_manage_repo_settings(Some(Permission::Developer), false));
    assert!(!can_manage_repo_settings(None, false));
    assert!(can_manage_repo_settings(None, true)); // global admin, no repo grant at all
}

#[test]
fn can_manage_repo_members_needs_owner_or_admin() {
    assert!(can_manage_repo_members(Some(Permission::Owner), false));
    assert!(!can_manage_repo_members(Some(Permission::Maintainer), false)); // settings power, not membership
    assert!(!can_manage_repo_members(None, false));
    assert!(can_manage_repo_members(None, true));
}
