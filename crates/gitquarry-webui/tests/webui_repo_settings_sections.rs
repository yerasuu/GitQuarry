use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_core::storage::remote::Manifest;
use gitquarry_webui::webui::repo_settings::{Namespaces, PageChrome, RepoState, render_settings_section};

fn render(section: &str) -> String {
    let config = RepoConfig::default();
    let manifest = Manifest::default();
    let users = [];
    let group_names = [];
    let granted = [];
    let mirrors = [];
    let mr_links = Default::default();
    render_settings_section(
        section,
        "acme/proj",
        &config,
        "acme",
        Namespaces {
            users: &users,
            group_names: &group_names,
        },
        RepoState {
            manifest: &manifest,
            granted: &granted,
            mirrors: &mirrors,
            mr_links: &mr_links,
        },
        PageChrome {
            csrf: "tok",
            message: None,
            pending_mirror_token: None,
            retry: None,
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .into_string()
}

#[test]
fn sub_nav_lists_every_section_with_its_own_label() {
    let html = render("settings");
    for label in [
        "repo settings",
        "default branch",
        "protected branches",
        "protected tags",
        "signed commits",
        "topics",
        "required reviewers",
        "labels",
        "size limit (LFS)",
        "per-user permissions",
        "external mirrors",
        "move repo",
        "delete repo",
    ] {
        assert!(html.contains(label), "sub-nav missing {label:?} in {html}");
    }
}

#[test]
fn bare_settings_shows_only_the_general_form() {
    let html = render("settings");
    assert!(html.contains("name=\"description\""), "{html}");
    assert!(!html.contains("name=\"labels\""), "{html}");
    assert!(!html.contains("name=\"new_local_name\""), "{html}");
}

#[test]
fn a_named_section_shows_only_its_own_form() {
    let html = render("settings_labels");
    assert!(html.contains("name=\"labels\""), "{html}");
    assert!(!html.contains("name=\"description\""), "{html}");
    assert!(!html.contains("name=\"new_local_name\""), "{html}");
}

#[test]
fn danger_zone_section_shows_only_its_own_form() {
    let html = render("settings_danger_zone");
    assert!(html.contains("name=\"confirm_name\""), "{html}");
    assert!(!html.contains("name=\"description\""), "{html}");
    assert!(!html.contains("name=\"labels\""), "{html}");
}

#[test]
fn unrecognized_section_falls_back_to_general() {
    assert_eq!(render("settings_nonexistent"), render("settings"));
}
