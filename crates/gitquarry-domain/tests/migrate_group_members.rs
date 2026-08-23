use gitquarry_core::auth::entities::{Group, Permission, Team};
use gitquarry_core::storage::uid::Uid;
use gitquarry_domain::commands::migrate_group_members::scoped_team_grants;
use std::collections::{BTreeMap, BTreeSet};

fn team(name: &str, members: &[&Uid], permission: Permission, repos: Option<&[&Uid]>) -> Team {
    Team {
        name: name.to_string(),
        members: members.iter().map(|&u| u.clone()).collect(),
        permission,
        repos: repos.map(|rs| rs.iter().map(|&r| r.clone()).collect()),
    }
}

/// A team that covered every repo in the group is already folded into the
/// member map by `Group::effective_members`; only a scoped one has anything
/// left that a group-wide role can't express.
#[test]
fn only_scoped_teams_produce_per_repo_grants() {
    let alice = Uid::new();
    let web = Uid::new();
    let group = Group {
        legacy_teams: vec![
            team("everyone", &[&alice], Permission::Developer, None),
            team("frontend", &[&alice], Permission::Maintainer, Some(&[&web])),
        ],
        ..Default::default()
    };
    assert_eq!(scoped_team_grants(&group), vec![(alice, web, Permission::Maintainer)]);
}

/// One triple per (member, repo) pair — the grant lives on each user's own
/// `User::repos`, so a team of two over two repos is four writes, not one.
#[test]
fn one_grant_per_member_and_repo() {
    let alice = Uid::new();
    let bob = Uid::new();
    let web = Uid::new();
    let api = Uid::new();
    let group = Group {
        legacy_teams: vec![team(
            "frontend",
            &[&alice, &bob],
            Permission::Reporter,
            Some(&[&web, &api]),
        )],
        ..Default::default()
    };
    assert_eq!(scoped_team_grants(&group).len(), 4);
}

#[test]
fn a_group_with_no_legacy_teams_produces_nothing() {
    let alice = Uid::new();
    let group = Group {
        members: BTreeMap::from([(alice.clone(), Permission::Owner)]),
        legacy_owners: BTreeSet::from([alice]),
        ..Default::default()
    };
    assert!(scoped_team_grants(&group).is_empty());
}
