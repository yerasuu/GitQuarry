use gitquarry_webui::webui::repo_settings::danger_zone::*;

#[test]
fn delete_repo_form_placeholders_the_full_name() {
    let html =
        render_delete_repo_form("team/project", None, "tok", None, None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"placeholder="team/project""#));
    assert!(html.contains(r#"value="delete_repo""#));
}

#[test]
fn deleted_repo_shows_restore_form_instead() {
    let html =
        render_delete_repo_form("team/project", Some(0), "tok", None, None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="restore_repo""#));
    assert!(!html.contains(r#"value="delete_repo""#));
}

#[test]
fn deleted_repo_also_offers_force_purge() {
    let html =
        render_delete_repo_form("team/project", Some(0), "tok", None, None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="purge_repo""#));
    assert!(html.contains(r#"placeholder="team/project""#));
}
