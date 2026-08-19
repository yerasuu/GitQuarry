use gitquarry_webui::webui::repo_settings::topics::*;

use gitquarry_core::storage::entities::RepoConfig;

#[test]
fn topics_form_shows_current_value() {
    let mut config = RepoConfig::default();
    config.topics.insert("rust".to_string());
    let html = render_topics_form("proyectoA", &config, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="rust""#));
    assert!(html.contains(r#"value="set_topics""#));
}
