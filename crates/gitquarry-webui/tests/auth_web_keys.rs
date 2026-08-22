//! `auth::web::keys`' rendered page — SSH and GPG keys, split out of
//! `security` along with the section itself.

use gitquarry_webui::auth::web::keys::*;

use gitquarry_core::auth::entities::User;

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

/// Regression: `add_pubkey`/`add_gpg_key` each target their own table's
/// `id`, same as the `remove_*` forms `render_ssh_keys_table`/
/// `render_gpg_keys_table` already emit — see `handler_keys_post`'s own
/// doc comment on why (a page-level fragment swapped into a table-only
/// target would corrupt the DOM). Losing either attribute quietly brings
/// back the browser's "confirm resubmission" dialog on F5.
#[test]
fn add_forms_target_their_own_table() {
    let html = render_keys_page(&base_user(), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(
        html.contains(r##"data-async data-async-target="#ssh-keys-table""##),
        "{html}"
    );
    assert!(
        html.contains(r##"data-async data-async-target="#gpg-keys-table""##),
        "{html}"
    );
}

#[test]
fn gpg_keys_section_shows_hint_when_empty() {
    let html = render_keys_page(&base_user(), "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("no keys yet"), "{html}");
    assert!(html.contains(r#"value="add_gpg_key""#));
}

#[test]
fn gpg_keys_section_lists_each_stored_key_by_fingerprint() {
    let mut user = base_user();
    user.gpg_keys = vec![gitquarry_core::auth::entities::GpgKeyEntry {
        armored: "totally-fake-armored-key-body-xyz".to_string(),
        fingerprint: "ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234".to_string(),
    }];
    let html = render_keys_page(&user, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234"), "{html}");
    assert!(html.contains(r#"value="remove_gpg_key""#));
    // The raw armored block itself never gets re-rendered — only the
    // already-computed fingerprint, same as `render_ssh_keys_table` never
    // echoes the raw pubkey line back either.
    assert!(!html.contains("totally-fake-armored-key-body-xyz"), "{html}");
}
