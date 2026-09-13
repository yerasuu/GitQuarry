use gitquarry_core::auth::entities::User;
use gitquarry_webui::auth::web::avatar::render_profile_section;

fn base_user() -> User {
    User {
        uid: Default::default(),
        username: "alice".to_string(),
        email: Some("alice@example.com".to_string()),
        pubkeys: vec![],
        gpg_keys: vec![],
        repos: Default::default(),
        default_permission: None,
        password_hash: None,
        is_admin: false,
        session_epoch: 0,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        totp_last_step: 0,
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: ["alt@example.com".to_string()].into_iter().collect(),
    }
}

/// Regression: `set_email` targets `#primary-email-section`,
/// `add_secondary_email` (alongside `remove_secondary_email`) targets
/// `#secondary-emails-table` — see `render_primary_email_section`'s own
/// doc comment on why neither can share the other's target (a page-level
/// fragment swapped into either would corrupt it, `style/app.js`'s
/// `target.replaceWith` taking the whole `<html>` as the replacement).
/// Losing either attribute quietly brings back the browser's "confirm
/// resubmission" dialog on F5.
#[test]
fn email_forms_target_their_own_section() {
    let html = render_profile_section(&base_user(), "tok", true, None, gitquarry_i18n::Locale::En).into_string();
    assert!(
        html.contains(r##"data-async data-async-target="#primary-email-section""##),
        "{html}"
    );
    assert_eq!(
        html.matches(r##"data-async data-async-target="#secondary-emails-table""##)
            .count(),
        2,
        "{html}"
    );
}
