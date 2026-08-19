use gitquarry_webui::auth::web::groups::*;
use std::collections::BTreeSet;
use std::collections::HashMap;

use gitquarry_core::auth::entities::{Group, Permission, Team};
use gitquarry_core::storage::uid::Uid;

#[test]
fn render_groups_page_echoes_the_search_query_in_the_input() {
    let root = Uid::new();
    let html = render_groups_page(
        &root,
        true,
        &[],
        &HashMap::new(),
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

/// `repos` labels double as deterministic test uids via `Uid`'s
/// `From<String>` — the matching `repo_names` map (built by
/// [`repo_names_with`]) has to use the same raw label as its key.
fn team(name: &str, members: &[&Uid], permission: Permission, repos: Option<&[&str]>) -> Team {
    Team {
        name: name.to_string(),
        members: members.iter().map(|&m| m.clone()).collect(),
        permission,
        repos: repos.map(|rs| rs.iter().map(|r| Uid::from(r.to_string())).collect()),
    }
}

fn names_with(uid: &Uid, username: &str) -> HashMap<Uid, String> {
    HashMap::from([(uid.clone(), username.to_string())])
}

fn repo_names_with(label: &str, full_name: &str) -> HashMap<Uid, String> {
    HashMap::from([(Uid::from(label.to_string()), full_name.to_string())])
}

#[test]
fn render_team_shows_scope_and_members() {
    let alice = Uid::new();
    let t = team("frontend", &[&alice], Permission::Developer, Some(&["web"]));
    let html = render_team(
        "acme",
        &t,
        &names_with(&alice, "alice"),
        &repo_names_with("web", "acme/web"),
        "tok",
        gitquarry_i18n::Locale::Es,
    )
    .into_string();
    assert!(html.contains("alice"));
    assert!(html.contains("desarrollador"));
    assert!(html.contains("web"));
    assert!(html.contains(r#"value="remove_team_member""#));
}

#[test]
fn render_team_shows_all_repos_when_unscoped() {
    let t = team("everyone", &[], Permission::Reporter, None);
    assert!(
        render_team(
            "acme",
            &t,
            &HashMap::new(),
            &HashMap::new(),
            "tok",
            gitquarry_i18n::Locale::Es
        )
        .into_string()
        .contains("todos los repos del grupo")
    );
}

#[test]
fn render_group_hides_delete_button_for_non_admin() {
    let group = Group {
        uid: Default::default(),
        name: "acme".to_string(),
        description: None,
        owners: BTreeSet::new(),
        teams: vec![],
        storage_quota_bytes: None,
    };
    assert!(
        render_group(
            &group,
            true,
            &HashMap::new(),
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
            false,
            &HashMap::new(),
            &HashMap::new(),
            "tok",
            gitquarry_i18n::Locale::Es
        )
        .into_string()
        .contains("delete_group")
    );
}

#[test]
fn groups_page_lists_only_groups_the_viewer_owns_unless_admin() {
    let alice = Uid::new();
    let root = Uid::new();
    let mut alice_owned = BTreeSet::new();
    alice_owned.insert(alice.clone());
    let groups = vec![
        Group {
            uid: Default::default(),
            name: "acme".to_string(),
            description: None,
            owners: alice_owned,
            teams: vec![],
            storage_quota_bytes: None,
        },
        Group {
            uid: Default::default(),
            name: "widgets".to_string(),
            description: None,
            owners: BTreeSet::new(),
            teams: vec![],
            storage_quota_bytes: None,
        },
    ];
    let names = names_with(&alice, "alice");
    let alice_view = render_groups_page(
        &alice,
        false,
        &groups,
        &names,
        &HashMap::new(),
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
        &HashMap::new(),
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
