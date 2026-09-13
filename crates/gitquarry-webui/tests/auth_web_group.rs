use gitquarry_core::auth::entities::{Group, Permission};
use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_webui::auth::web::group::render_group_page;
use std::collections::{BTreeMap, HashMap};

fn group_with(name: &str, members: &[(&Uid, Permission)]) -> Group {
    Group {
        uid: Uid::new(),
        name: name.to_string(),
        members: members.iter().map(|(u, p)| ((*u).clone(), *p)).collect(),
        ..Default::default()
    }
}

fn repo(name: &str, visibility: Visibility) -> (String, RepoConfig) {
    (
        name.to_string(),
        RepoConfig {
            full_name: name.to_string(),
            visibility,
            ..Default::default()
        },
    )
}

#[test]
fn renders_the_groups_repos_and_subgroups() {
    let acme = group_with("acme", &[]);
    let team = group_with("acme/team", &[]);
    let html = render_group_page(
        &acme,
        std::slice::from_ref(&acme),
        &[repo("acme/web", Visibility::Public)],
        std::slice::from_ref(&team),
        &HashMap::new(),
        false,
        false,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains(r#"href="/acme/web""#));
    assert!(html.contains(r#"href="/_group/acme/team""#));
}

/// Who belongs to a group is only shown to the group itself — same reasoning
/// its private repos aren't listed to an outsider either.
#[test]
fn hides_the_member_list_from_a_non_member() {
    let alice = Uid::new();
    let acme = group_with("acme", &[(&alice, Permission::Owner)]);
    let names = HashMap::from([(alice.clone(), "alice".to_string())]);

    let outsider = render_group_page(
        &acme,
        std::slice::from_ref(&acme),
        &[],
        &[],
        &names,
        false,
        false,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(!outsider.contains("alice"));

    let member = render_group_page(
        &acme,
        std::slice::from_ref(&acme),
        &[],
        &[],
        &names,
        true,
        false,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(member.contains("alice"));
}

/// A subgroup's page has to show the members it inherited — that inheritance
/// is what subgroups are for — but mark them, or the page reads as though
/// this group granted access it never did.
#[test]
fn marks_a_member_whose_role_comes_from_a_parent() {
    let alice = Uid::new();
    let bob = Uid::new();
    let acme = group_with("acme", &[(&alice, Permission::Developer)]);
    let team = Group {
        uid: Uid::new(),
        name: "acme/team".to_string(),
        members: BTreeMap::from([(bob.clone(), Permission::Maintainer)]),
        parent: Some(acme.uid.clone()),
        ..Default::default()
    };
    let names = HashMap::from([(alice.clone(), "alice".to_string()), (bob.clone(), "bob".to_string())]);
    let html = render_group_page(
        &team,
        &[acme, team.clone()],
        &[],
        &[],
        &names,
        true,
        false,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("alice"));
    assert!(html.contains("bob"));
    assert_eq!(html.matches("(inherited)").count(), 1);
}

/// The breadcrumb is the only way back up from a subgroup page.
#[test]
fn links_every_ancestor_in_the_breadcrumb() {
    let acme = group_with("acme", &[]);
    let team = group_with("acme/team", &[]);
    let html = render_group_page(
        &team,
        &[acme, team.clone()],
        &[],
        &[],
        &HashMap::new(),
        false,
        false,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains(r#"href="/_group/acme""#));
    // The page's own group is the last crumb, not a link to itself.
    assert!(!html.contains(r#"href="/_group/acme/team""#));
}

#[test]
fn offers_a_manage_link_only_to_someone_who_may_manage() {
    let acme = group_with("acme", &[]);
    let plain = render_group_page(
        &acme,
        std::slice::from_ref(&acme),
        &[],
        &[],
        &HashMap::new(),
        true,
        false,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(!plain.contains(r#"href="/_groups""#));

    let manager = render_group_page(
        &acme,
        std::slice::from_ref(&acme),
        &[],
        &[],
        &HashMap::new(),
        true,
        true,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(manager.contains(r#"href="/_groups""#));
}
