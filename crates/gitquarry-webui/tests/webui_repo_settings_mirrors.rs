use gitquarry_mirror_external::access::{
    ExternalMirror, ExternalMirrorProvider, ExternalPrLink, RefSyncStatus, SyncDirection,
};
use gitquarry_webui::webui::repo_settings::mirrors::*;
use std::collections::HashMap;

fn sample_mirror(id: &str) -> ExternalMirror {
    ExternalMirror {
        id: id.to_string(),
        remote_url: "https://github.com/acme/demo.git".to_string(),
        token: "ghp_secret".to_string(),
        provider: ExternalMirrorProvider::GitHub,
        direction: SyncDirection::Bidirectional,
        enabled: true,
        label: "backup".to_string(),
        ..Default::default()
    }
}

#[test]
fn empty_list_shows_hint_and_add_form() {
    let no_links = HashMap::new();
    let html =
        render_mirrors_section("acme/demo", &[], &no_links, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("no mirrors configured yet"));
    assert!(html.contains(r#"value="add_mirror""#));
}

#[test]
fn lists_a_mirror_row_with_sync_and_delete_actions() {
    let mirror = (sample_mirror("m1"), "etag1".to_string());
    let no_links = HashMap::new();
    let html = render_mirrors_section(
        "acme/demo",
        &[mirror],
        &no_links,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("backup"));
    assert!(html.contains("GitHub"));
    assert!(html.contains(r#"value="sync_mirror_now""#));
    assert!(html.contains(r#"value="delete_mirror""#));
    assert!(html.contains(r#"value="edit_mirror""#));
    // The real token is never rendered outside the one-time banner.
    assert!(!html.contains("ghp_secret"));
}

#[test]
fn edit_form_prefills_branch_patterns_as_comma_separated() {
    let mirror = ExternalMirror {
        branch_patterns: std::collections::BTreeSet::from(["main".to_string(), "release/*".to_string()]),
        ..sample_mirror("m1")
    };
    let no_links = HashMap::new();
    let html = render_mirrors_section(
        "acme/demo",
        &[(mirror, "etag1".to_string())],
        &no_links,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains(r#"name="branch_patterns" value="main, release/*""#));
}

#[test]
fn add_form_starts_with_an_empty_branch_patterns_field() {
    let no_links = HashMap::new();
    let html =
        render_mirrors_section("acme/demo", &[], &no_links, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"name="branch_patterns" value="""#));
}

#[test]
fn edit_form_carries_the_mirrors_real_etag_as_its_cas_guard() {
    let mirror = (sample_mirror("m1"), "real-etag-123".to_string());
    let no_links = HashMap::new();
    let html = render_mirrors_section(
        "acme/demo",
        &[mirror],
        &no_links,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains(r#"name="mirror_etag" value="real-etag-123""#));
}

#[test]
fn pending_token_is_shown_once_for_the_matching_mirror() {
    let mirror = (sample_mirror("m1"), "etag1".to_string());
    let pending = ("m1".to_string(), "ghp_brand_new".to_string());
    let no_links = HashMap::new();
    let html = render_mirrors_section(
        "acme/demo",
        &[mirror],
        &no_links,
        "tok",
        Some(&pending),
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("ghp_brand_new"));
    assert!(html.contains("backup"));
}

#[test]
fn ref_status_pills_render_each_ref_with_its_own_status() {
    let mut mirror = sample_mirror("m1");
    mirror
        .ref_status
        .insert("refs/heads/main".to_string(), RefSyncStatus::InSync);
    mirror
        .ref_status
        .insert("refs/heads/dev".to_string(), RefSyncStatus::Diverged);
    let no_links = HashMap::new();
    let html = render_mirrors_section(
        "acme/demo",
        &[(mirror, "etag1".to_string())],
        &no_links,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("refs/heads/main"));
    assert!(html.contains("in sync"));
    assert!(html.contains("refs/heads/dev"));
    assert!(html.contains("diverged"));
}

#[test]
fn a_whole_mirror_failure_shows_with_its_message_in_the_tooltip() {
    let mut mirror = sample_mirror("m1");
    mirror.last_error = Some("couldn't reach the remote".to_string());
    let no_links = HashMap::new();
    let html = render_mirrors_section(
        "acme/demo",
        &[(mirror, "etag1".to_string())],
        &no_links,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("sync failed"), "{html}");
    assert!(html.contains(r#"title="couldn't reach the remote""#), "{html}");
    // The "nothing synced yet" dash is for a mirror with no status at
    // all, not one that just failed outright.
    assert!(!html.contains("<em>—</em>"), "{html}");
}

#[test]
fn shows_linked_prs_only_for_a_mirror_with_mr_sync_on() {
    let mut mirror = sample_mirror("m1");
    mirror.sync_merge_requests = true;
    let mut links = HashMap::new();
    links.insert(
        "m1".to_string(),
        vec![ExternalPrLink {
            local_number: 3,
            external_number: 7,
            remote_updated_cursor: "2024-01-01T00:00:00Z".to_string(),
            ..Default::default()
        }],
    );
    let html = render_mirrors_section(
        "acme/demo",
        &[(mirror, "etag1".to_string())],
        &links,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("linked merge requests"));
    assert!(html.contains("#3"));
    assert!(html.contains("#7"));
}

#[test]
fn hides_the_linked_prs_section_when_mr_sync_is_off() {
    let mirror = sample_mirror("m1"); // sync_merge_requests defaults to false
    let no_links = HashMap::new();
    let html = render_mirrors_section(
        "acme/demo",
        &[(mirror, "etag1".to_string())],
        &no_links,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(!html.contains("linked merge requests"));
}
