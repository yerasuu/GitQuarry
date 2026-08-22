use gitquarry_webui::auth::web::appearance::*;

use gitquarry_core::auth::entities::User;

fn user_with_prefs(text_size_default: &str, theme_default: &str) -> User {
    user_with_locale(text_size_default, theme_default, "")
}

fn user_with_locale(text_size_default: &str, theme_default: &str, locale_default: &str) -> User {
    User {
        uid: Default::default(),
        username: "alice".to_string(),
        email: None,
        pubkeys: vec![],
        gpg_keys: vec![],
        repos: Default::default(),
        default_permission: None,
        password_hash: None,
        is_admin: false,
        session_epoch: 0,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: text_size_default.to_string(),
        theme_default: theme_default.to_string(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        totp_last_step: 0,
        locale_default: locale_default.to_string(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    }
}

#[test]
fn text_size_pref_shows_the_current_default_as_selected() {
    let html =
        render_appearance_page(&user_with_prefs("sm", ""), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="sm" selected>small</option>"#));
    assert!(html.contains(r#"<option value="md">medium</option>"#));
    assert!(html.contains(r#"<option value="lg">large</option>"#));

    let html =
        render_appearance_page(&user_with_prefs("lg", ""), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="lg" selected>large</option>"#));
}

#[test]
fn theme_pref_shows_the_current_default_as_selected() {
    let html = render_appearance_page(&user_with_prefs("", ""), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="" selected>automatic (follows the system)</option>"#));
    assert!(html.contains(r#"<option value="light">light</option>"#));
    assert!(html.contains(r#"<option value="dark">dark</option>"#));

    let html =
        render_appearance_page(&user_with_prefs("", "dark"), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="dark" selected>dark</option>"#));
}

#[test]
fn locale_pref_shows_the_current_default_as_selected() {
    let html =
        render_appearance_page(&user_with_locale("", "", ""), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="" selected>automatic (uses the site's built-in default)</option>"#));
    assert!(html.contains(r#"<option value="es">Español</option>"#));
    assert!(html.contains(r#"<option value="en">English</option>"#));

    let html =
        render_appearance_page(&user_with_locale("", "", "es"), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="es" selected>Español</option>"#));
}
