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

/// Regression: 13 sections wrapped badly (or forced `table.tabs`'s own
/// horizontal scroll) in the old horizontal `project_subnav` row — the
/// sidebar reuses `account_settings_page`'s own `.settings-layout`/
/// `.settings-sidebar`/`.settings-content` classes instead (see
/// `render_settings_section`'s own doc comment), so losing any of them
/// quietly brings back the old overflow.
#[test]
fn sections_render_as_a_sidebar_not_a_horizontal_row() {
    let html = render("settings_labels");
    assert!(html.contains(r#"<div class="settings-layout">"#), "{html}");
    assert!(html.contains(r#"<nav class="settings-sidebar""#), "{html}");
    assert!(html.contains(r#"<div class="settings-content">"#), "{html}");
    assert!(!html.contains(r#"<table class="tabs subnav">"#), "{html}");
    assert!(
        html.contains(r#"<a class="active" aria-current="page" href="/acme/proj?view=settings_labels">labels</a>"#),
        "{html}"
    );
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

/// Regression: the general/topics/labels sub-forms all target
/// `#repo-settings-body` (see `render_settings_section`'s own doc
/// comment) — losing that quietly brings back the browser's "confirm
/// resubmission" dialog on F5. `settings_move`/`settings_danger_zone` are
/// deliberately excluded: their success path is a real `Redirect` (`move_repo`/
/// `delete_repo`/`purge_repo`/`restore_repo` in `handler_repo_conf_post`),
/// and `fetch()` follows a redirect transparently — making those forms
/// `data-async` would swap the *redirect target's* markup into
/// `#repo-settings-body` instead of leaving the dialog risk on their own
/// (rare, already name-confirmed) failure path alone.
#[test]
fn every_section_wraps_itself_and_only_the_redirecting_ones_skip_async() {
    for section in ["settings", "settings_topics", "settings_labels"] {
        let html = render(section);
        assert!(
            html.starts_with(r#"<div id="repo-settings-body">"#),
            "{section}: {html}"
        );
        assert!(
            html.contains(r##"data-async data-async-target="#repo-settings-body""##),
            "{section}: {html}"
        );
    }
    for section in ["settings_move", "settings_danger_zone"] {
        let html = render(section);
        assert!(
            !html.contains(r##"data-async-target="#repo-settings-body""##),
            "{section}: {html}"
        );
    }
}
