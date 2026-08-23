use gitquarry_webui::auth::web::groups::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use gitquarry_core::auth::entities::{Group, Permission, Team};
use gitquarry_core::storage::uid::Uid;

fn names_with(uid: &Uid, username: &str) -> HashMap<Uid, String> {
    HashMap::from([(uid.clone(), username.to_string())])
}

fn group_with(name: &str, members: &[(&Uid, Permission)]) -> Group {
    Group {
        name: name.to_string(),
        members: members.iter().map(|(u, p)| ((*u).clone(), *p)).collect(),
        ..Default::default()
    }
}

#[test]
fn render_groups_page_echoes_the_search_query_in_the_input() {
    let root = Uid::new();
    let html = render_groups_page(
        &root,
        true,
        &[],
        &HashMap::new(),
        "acme",
        PageChrome {
            csrf: "tok",
            message: None,
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .into_string();
    assert!(html.contains(r#"input name="q" value="acme""#));
}

#[test]
fn render_group_hides_delete_button_for_non_admin() {
    let group = group_with("acme", &[]);
    let viewer = Uid::new();
    assert!(
        render_group(
            &group,
            &viewer,
            true,
            &HashMap::new(),
            "tok",
            gitquarry_i18n::Locale::Es
        )
        .into_string()
        .contains("delete_group")
    );
    assert!(
        !render_group(
            &group,
            &viewer,
            false,
            &HashMap::new(),
            "tok",
            gitquarry_i18n::Locale::Es
        )
        .into_string()
        .contains("delete_group")
    );
}

/// A member's tier is shown to everyone in the group, but only a viewer who
/// may manage it gets the forms that change it — otherwise a `Developer`
/// would be shown controls every submit is going to reject with a 403.
#[test]
fn render_group_shows_member_forms_only_to_a_manager() {
    let alice = Uid::new();
    let bob = Uid::new();
    let group = group_with("acme", &[(&alice, Permission::Owner), (&bob, Permission::Developer)]);
    let names = HashMap::from([(alice.clone(), "alice".to_string()), (bob.clone(), "bob".to_string())]);

    let owner_view = render_group(&group, &alice, false, &names, "tok", gitquarry_i18n::Locale::En).into_string();
    assert!(owner_view.contains(r#"value="set_member""#));
    assert!(owner_view.contains(r#"value="remove_member""#));

    let member_view = render_group(&group, &bob, false, &names, "tok", gitquarry_i18n::Locale::En).into_string();
    assert!(member_view.contains("bob"));
    assert!(!member_view.contains(r#"value="set_member""#));
    assert!(!member_view.contains(r#"value="remove_member""#));
}

/// The legacy fields still grant access until `migrate-group-members` runs,
/// and a team scoped to only some repos can't be shown as a group-wide role
/// — so the page has to say so rather than render a member table that looks
/// complete but isn't.
#[test]
fn render_group_warns_while_legacy_owners_or_teams_remain() {
    let alice = Uid::new();
    let mut group = group_with("acme", &[(&alice, Permission::Owner)]);
    let clean = render_group(
        &group,
        &alice,
        false,
        &names_with(&alice, "alice"),
        "tok",
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(!clean.contains("migrate-group-members"));

    group.legacy_teams = vec![Team {
        name: "frontend".to_string(),
        members: BTreeSet::from([alice.clone()]),
        permission: Permission::Developer,
        repos: Some(BTreeSet::from([Uid::from("acme/web".to_string())])),
    }];
    let stale = render_group(
        &group,
        &alice,
        false,
        &names_with(&alice, "alice"),
        "tok",
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(stale.contains("migrate-group-members"));
}

/// Regression: every form on this page targets the same `#groups-list`.
/// Losing any one of them quietly brings back the browser's "confirm
/// resubmission" dialog on F5 for that specific action.
#[test]
fn every_group_form_targets_groups_list() {
    let alice = Uid::new();
    let group = group_with("acme", &[(&alice, Permission::Owner)]);
    let html = render_group(
        &group,
        &alice,
        true,
        &names_with(&alice, "alice"),
        "tok",
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    // set_description, set_storage_quota, set_member (row), remove_member
    // (row), set_member (add), delete_group.
    assert_eq!(
        html.matches(r##"data-async data-async-target="#groups-list""##).count(),
        6,
        "{html}"
    );
}

/// Membership, not ownership, is what puts a group on this page: a
/// `Developer` has to be able to find the group they belong to. Anything
/// they aren't a member of stays hidden unless they're a global admin.
#[test]
fn groups_page_lists_the_groups_the_viewer_belongs_to_unless_admin() {
    let alice = Uid::new();
    let root = Uid::new();
    let groups = vec![
        Group {
            name: "acme".to_string(),
            members: BTreeMap::from([(alice.clone(), Permission::Developer)]),
            ..Default::default()
        },
        Group {
            name: "widgets".to_string(),
            ..Default::default()
        },
    ];
    let names = names_with(&alice, "alice");
    let alice_view = render_groups_page(
        &alice,
        false,
        &groups,
        &names,
        "",
        PageChrome {
            csrf: "tok",
            message: None,
            locale: gitquarry_i18n::Locale::Es,
        },
    )
    .into_string();
    assert!(alice_view.contains("acme"));
    assert!(!alice_view.contains("widgets"));

    let admin_view = render_groups_page(
        &root,
        true,
        &groups,
        &names,
        "",
        PageChrome {
            csrf: "tok",
            message: None,
            locale: gitquarry_i18n::Locale::Es,
        },
    )
    .into_string();
    assert!(admin_view.contains("acme"));
    assert!(admin_view.contains("widgets"));
}
