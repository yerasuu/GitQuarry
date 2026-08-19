use gitquarry_core::auth::access::permissions::*;
use gitquarry_core::auth::entities::Team;
use gitquarry_core::auth::entities::{Group, Permission, User};
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
        is_federation_moderator: false,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    }
}

fn ruid(label: &str) -> Uid {
    Uid::from(label.to_string())
}

/// `members`/`owners` below take `&User`, not raw name strings, now
/// that `Team::members`/`Group::owners` are uid-keyed — a test needs an
/// actual `User` in hand to reference its uid. `repos` labels double as
/// deterministic test uids via [`ruid`], same as `user_with_repos` —
/// pass full repo-path labels (`"acme/web"`), not group-local ones:
/// `Team::repos` holds full repo uids now, not names relative to the
/// group's own namespace.
fn team(name: &str, members: &[&User], permission: Permission, repos: Option<&[&str]>) -> Team {
    Team {
        name: name.to_string(),
        members: members.iter().map(|u| u.uid.clone()).collect(),
        permission,
        repos: repos.map(|rs| rs.iter().map(|r| ruid(r)).collect()),
    }
}

fn group(name: &str, owners: &[&User], teams: Vec<Team>) -> Group {
    Group {
        uid: Default::default(),
        name: name.to_string(),
        description: None,
        owners: owners.iter().map(|u| u.uid.clone()).collect(),
        teams,
        storage_quota_bytes: None,
    }
}

#[test]
fn effective_permission_prefers_explicit_repo_grant_over_group() {
    let alice = user_with_repos("alice", &[("acme/proj", Permission::Developer)]);
    let acme = group(
        "acme",
        &[],
        vec![team("readers", &[&alice], Permission::Reporter, None)],
    );
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

#[test]
fn effective_permission_owner_gets_implicit_owner_without_a_team() {
    let alice = user_with_repos("alice", &[]);
    let acme = group("acme", &[&alice], vec![]);
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/proj", &ruid("acme/proj")),
        Some(Permission::Owner)
    );
}

#[test]
fn effective_permission_falls_back_to_team_membership() {
    let alice = user_with_repos("alice", &[]);
    let acme = group(
        "acme",
        &[],
        vec![team("everyone", &[&alice], Permission::Developer, None)],
    );
    // No explicit grant on "acme/proj", but alice's team covers every
    // repo in the group (repos: None).
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/proj", &ruid("acme/proj")),
        Some(Permission::Developer)
    );
    // Same group, but bob isn't on any team and has no default permission either.
    let bob = user_with_repos("bob", &[]);
    assert_eq!(
        effective_permission(&bob, Some(&acme), "acme/proj", &ruid("acme/proj")),
        None
    );
}

#[test]
fn effective_permission_team_scoped_to_specific_repos_only() {
    let alice = user_with_repos("alice", &[]);
    let acme = group(
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
fn effective_permission_picks_the_highest_of_several_teams() {
    let alice = user_with_repos("alice", &[]);
    let acme = group(
        "acme",
        &[],
        vec![
            team("readers", &[&alice], Permission::Reporter, None),
            team("writers", &[&alice], Permission::Developer, Some(&["acme/proj"])),
        ],
    );
    assert_eq!(
        effective_permission(&alice, Some(&acme), "acme/proj", &ruid("acme/proj")),
        Some(Permission::Developer)
    );
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

#[test]
fn can_manage_group_allows_admin_and_owner_only() {
    let mut root = user_with_repos("root", &[]);
    root.is_admin = true;
    let alice = user_with_repos("alice", &[]);
    let bob = user_with_repos("bob", &[]);
    let acme = group("acme", &[&alice], vec![]);
    assert!(can_manage_group(Some(&root), Some(&acme))); // global admin
    assert!(can_manage_group(Some(&alice), Some(&acme))); // listed owner
    assert!(!can_manage_group(Some(&bob), Some(&acme))); // neither
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
