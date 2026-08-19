use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_webui::webui::repo_settings::lfs_limit::*;

#[test]
fn lfs_limit_form_shows_current_value() {
    let config = RepoConfig {
        lfs_max_blob_size: Some("50M".to_string()),
        ..Default::default()
    };
    let html = render_lfs_limit_form("proyectoA", &config, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="50M""#));
    assert!(html.contains(r#"value="set_lfs_max_blob_size""#));
}

#[test]
fn lfs_limit_form_blank_when_unset() {
    let config = RepoConfig::default();
    let html = render_lfs_limit_form("proyectoA", &config, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"name="lfs_max_blob_size" value="""#));
}
