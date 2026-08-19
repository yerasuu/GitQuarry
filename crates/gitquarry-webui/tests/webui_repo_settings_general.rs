use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_webui::webui::repo_settings::general::*;

/// Regression: `owner` used to be a free `<select>` any admin could
/// point at an unrelated user/group, independent of the repo's actual
/// namespace — `save_conf` now always re-derives it from the repo's
/// own namespace instead, so there's nothing left to submit here.
#[test]
fn owner_is_shown_read_only_not_as_a_select() {
    let config = RepoConfig::default();
    let html = render_repo_conf_form(
        "acme/proj",
        &config,
        "acme",
        "tok",
        None,
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(!html.contains("<select name=\"owner\""), "{html}");
    assert!(html.contains(">acme "), "{html}");
    assert!(html.contains("move the repo below"), "{html}");
}

#[test]
fn is_template_checkbox_reflects_the_repos_current_flag() {
    let config = RepoConfig {
        is_template: true,
        ..Default::default()
    };
    let html = render_repo_conf_form(
        "acme/proj",
        &config,
        "acme",
        "tok",
        None,
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(
        html.contains(r#"input type="checkbox" name="is_template" value="1" checked"#),
        "{html}"
    );
}

#[test]
fn is_template_checkbox_is_unchecked_by_default() {
    let config = RepoConfig::default();
    let html = render_repo_conf_form(
        "acme/proj",
        &config,
        "acme",
        "tok",
        None,
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(
        html.contains(r#"input type="checkbox" name="is_template" value="1">"#),
        "{html}"
    );
}
