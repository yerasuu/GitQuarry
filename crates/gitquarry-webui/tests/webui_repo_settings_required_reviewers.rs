use gitquarry_webui::webui::repo_settings::required_reviewers::*;

use gitquarry_core::storage::entities::RepoConfig;

#[test]
fn required_reviewers_form_shows_current_value() {
    let mut config = RepoConfig::default();
    config.required_reviewers.insert("alice".to_string());
    let html =
        render_required_reviewers_form("team/proj", &config, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="alice""#));
    assert!(html.contains(r#"value="set_required_reviewers""#));
}

#[test]
fn required_reviewers_form_empty_without_any_set() {
    let config = RepoConfig::default();
    let html =
        render_required_reviewers_form("team/proj", &config, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"name="required_reviewers" value="""#));
}
