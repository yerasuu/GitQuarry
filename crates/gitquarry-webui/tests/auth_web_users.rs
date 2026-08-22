use gitquarry_webui::auth::web::users::*;
use std::collections::BTreeMap;

use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::storage::uid::Uid;
use std::collections::HashMap;

#[test]
fn render_users_page_echoes_the_search_query_in_the_input() {
    let html = render_users_page(&[], &HashMap::new(), "tok", None, "ali", gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"input name="q" value="ali""#));
}

#[test]
fn render_user_lists_each_repo_with_a_remove_button() {
    let proyecto_a = Uid::from("projectA".to_string());
    let mut repos = BTreeMap::new();
    repos.insert(proyecto_a.clone(), Permission::Developer);
    let repo_names = HashMap::from([(proyecto_a, "projectA".to_string())]);
    let user = User {
        uid: Default::default(),
        username: "alice".to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos,
        default_permission: Some(Permission::Reporter),
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
    };
    let html = render_user(&user, &repo_names, "tok", gitquarry_i18n::Locale::Es).into_string();
    assert!(html.contains("<code>projectA</code>"));
    assert!(html.contains(">desarrollador<"));
    assert!(html.contains("<code>*</code>"));
    assert!(html.contains(">lector<"));
    assert!(html.contains(r#"value="remove_repo_perm""#));
}

#[test]
fn render_user_shows_empty_hint_without_repos() {
    let user = User {
        uid: Default::default(),
        username: "bob".to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: BTreeMap::new(),
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
    };
    assert!(
        render_user(&user, &HashMap::new(), "tok", gitquarry_i18n::Locale::Es)
            .into_string()
            .contains("sin repos todavia")
    );
}

/// Regression: every action whose whole effect stays inside one user's own
/// card (`add_pubkey`, `add_repo_perm`, `set_email`, `set_password`,
/// `set_storage_quota`, `toggle_admin` — none of these had `data-async` at
/// all before `handler_users_post` learned to async-render them alongside
/// `remove_pubkey`/`remove_repo_perm`/`disable_totp`) targets that card's
/// own `id`; `delete_user` targets `#users-list` instead, since it changes
/// which cards exist. Losing either quietly brings back the browser's
/// "confirm resubmission" dialog on F5 for that specific action.
#[test]
fn card_actions_and_delete_target_the_right_fragment() {
    let bob = User {
        uid: Default::default(),
        username: "bob".to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: BTreeMap::new(),
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
    };
    let html = render_user(&bob, &HashMap::new(), "tok", gitquarry_i18n::Locale::En).into_string();
    assert_eq!(
        html.matches(r##"data-async data-async-target="#user-card-bob""##)
            .count(),
        6,
        "{html}"
    );
    assert_eq!(
        html.matches(r##"data-async data-async-target="#users-list""##).count(),
        1,
        "{html}"
    );
}

#[test]
fn render_user_offers_admin_totp_disable_only_when_enabled() {
    let mut user = User {
        uid: Default::default(),
        username: "carol".to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: BTreeMap::new(),
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
    };
    assert!(
        !render_user(&user, &HashMap::new(), "tok", gitquarry_i18n::Locale::Es)
            .into_string()
            .contains(r#"value="disable_totp""#)
    );

    user.totp_enabled = true;
    user.totp_secret = Some("JBSWY3DPEHPK3PXP".to_string());
    assert!(
        render_user(&user, &HashMap::new(), "tok", gitquarry_i18n::Locale::Es)
            .into_string()
            .contains(r#"value="disable_totp""#)
    );
}
