use gitquarry_core::storage::entities::{Label, RepoConfig};
use gitquarry_webui::webui::repo_settings::labels::*;

#[test]
fn labels_form_shows_current_value_and_pills() {
    let mut config = RepoConfig::default();
    config.labels.push(Label {
        name: "bug".to_string(),
        color: "#d73a4a".to_string(),
    });
    let html = render_labels_form("proyectoA", &config, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="bug:#d73a4a""#));
    assert!(html.contains(r#"value="set_labels""#));
    assert!(html.contains("#d73a4a"));
}
