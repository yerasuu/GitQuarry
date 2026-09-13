use gitquarry_webui::auth::web::security::*;

use gitquarry_core::auth::entities::User;

#[test]
fn security_page_never_exposes_admin_or_delete_actions() {
    let user = User {
        uid: Default::default(),
        username: "alice".to_string(),
        email: Some("alice@example.com".to_string()),
        pubkeys: vec![],
        gpg_keys: vec![],
        repos: Default::default(),
        default_permission: None,
        password_hash: None,
        is_admin: true,
        session_epoch: 0,
        is_federation_moderator: false,
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
        secondary_emails: Default::default(),
    };
    let html = render_security_page(&user, "tok", None, None, None, gitquarry_i18n::Locale::En).into_string();
    // No self-service toggle for the flag that grants /_settings + /_users.
    assert!(!html.contains("toggle_admin"));
    assert!(!html.contains("delete_user"));
    // Regression: every form on this page (set_password here, plus
    // whichever 2FA form the current state shows) targets one
    // `#security-body` wrapper — a submit can reveal a TOTP secret/QR or a
    // recovery-code set only the *next* render shows, so it has to refresh
    // the whole page, not just its own form. Losing this quietly brings
    // back the browser's "confirm resubmission" dialog on F5.
    assert!(html.starts_with(r#"<div id="security-body">"#), "{html}");
    assert_eq!(
        html.matches(r##"data-async data-async-target="#security-body""##)
            .count(),
        2,
        "{html}"
    );
    // Own identity is shown but never editable here: the address moved to
    // `/_account/profile` entirely, and the only `username` control left is
    // the hidden one a password manager reads to know which account the
    // new password belongs to.
    assert!(html.contains("alice@example.com"));
    assert!(!html.contains(r#"name="email""#));
    assert!(!html.contains(r#"<input name="username""#));
    assert!(
        html.contains(r#"input type="hidden" name="username" value="alice""#),
        "{html}"
    );
}

fn base_user() -> User {
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
        is_federation_moderator: false,
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
        secondary_emails: Default::default(),
    }
}

#[test]
fn totp_section_offers_activation_when_nothing_is_set() {
    let html = render_security_page(&base_user(), "tok", None, None, None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="enable_totp""#));
    assert!(!html.contains(r#"value="confirm_totp""#));
    assert!(!html.contains(r#"value="disable_totp""#));
}

#[test]
fn totp_section_shows_the_pending_secret_and_confirm_form() {
    let html = render_security_page(
        &base_user(),
        "tok",
        None,
        Some("JBSWY3DPEHPK3PXP"),
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("JBSWY3DPEHPK3PXP"));
    assert!(html.contains(r#"value="confirm_totp""#));
    assert!(!html.contains(r#"value="enable_totp""#));
    assert!(!html.contains(r#"value="disable_totp""#));
}

#[test]
fn totp_section_offers_disable_once_enabled_and_hides_enable_form() {
    let mut user = base_user();
    user.totp_enabled = true;
    user.totp_secret = Some("JBSWY3DPEHPK3PXP".to_string());
    let html = render_security_page(&user, "tok", None, None, None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"value="disable_totp""#));
    assert!(!html.contains(r#"value="enable_totp""#));
    assert!(!html.contains(r#"value="confirm_totp""#));
    // The secret itself is never re-rendered once enabled.
    assert!(!html.contains("JBSWY3DPEHPK3PXP"));
}

#[test]
fn recovery_codes_are_shown_once_then_never_again() {
    let mut user = base_user();
    user.totp_enabled = true;
    user.totp_secret = Some("JBSWY3DPEHPK3PXP".to_string());
    let codes = vec!["aaaa-bbbb-cccc-dddd".to_string()];

    let html = render_security_page(&user, "tok", None, None, Some(&codes), gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("aaaa-bbbb-cccc-dddd"));
    assert!(html.contains(r#"value="regenerate_recovery_codes""#));

    // Without a fresh reveal, the (never-stored) plaintext just isn't there.
    let html = render_security_page(&user, "tok", None, None, None, gitquarry_i18n::Locale::En).into_string();
    assert!(!html.contains("aaaa-bbbb-cccc-dddd"));
    assert!(html.contains(r#"value="regenerate_recovery_codes""#));
}

#[test]
fn recovery_codes_section_is_absent_before_2fa_is_enabled() {
    let html = render_security_page(&base_user(), "tok", None, None, None, gitquarry_i18n::Locale::En).into_string();
    assert!(!html.contains(r#"value="regenerate_recovery_codes""#));
}
