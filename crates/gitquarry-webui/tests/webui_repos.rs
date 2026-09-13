use gitquarry_core::auth::entities::Group;
use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_webui::webui::repos::{render_groups_tab, render_shared_tab, render_yours_tab};

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
fn yours_tab_lists_personal_repos_including_private_ones() {
    let html = render_yours_tab(
        &[
            repo("alice/web", Visibility::Public),
            repo("alice/secret", Visibility::Private),
        ],
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains(r#"href="/alice/web""#));
    assert!(html.contains(r#"href="/alice/secret""#));
    assert!(html.contains("private"));
}

#[test]
fn yours_tab_says_so_when_there_is_nothing() {
    let html = render_yours_tab(&[], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("don't own any repo yet"));
}

/// The split is the point of this page: each group gets its own section
/// headed by a link to that group, and a subgroup is its own section under
/// its full path rather than folded into the parent's — separate namespace,
/// separate repos.
#[test]
fn groups_tab_keeps_one_section_per_group_linked_to_its_page() {
    let acme = Group {
        uid: Uid::new(),
        name: "acme".to_string(),
        ..Default::default()
    };
    let team = Group {
        uid: Uid::new(),
        name: "acme/team".to_string(),
        ..Default::default()
    };
    let html = render_groups_tab(
        &[
            (acme, vec![repo("acme/web", Visibility::Private)]),
            (team, vec![repo("acme/team/api", Visibility::Public)]),
        ],
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains(r#"href="/_group/acme""#));
    assert!(html.contains(r#"href="/_group/acme/team""#));
    assert!(html.contains(r#"href="/acme/web""#));
    assert!(html.contains(r#"href="/acme/team/api""#));
}

#[test]
fn groups_tab_says_so_when_the_viewer_has_no_group() {
    let html = render_groups_tab(&[], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("don't belong to any group"));
}

#[test]
fn shared_tab_lists_individually_granted_repos() {
    let html = render_shared_tab(&[repo("bob/tool", Visibility::Private)], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"href="/bob/tool""#));

    let empty = render_shared_tab(&[], gitquarry_i18n::Locale::En).into_string();
    assert!(empty.contains("nobody granted you access"));
}
