use gitquarry_core::auth::entities::Permission;
use gitquarry_core::auth::entities::User;
use gitquarry_webui::webui::repo_settings::move_repo::*;
use std::collections::BTreeMap;

/// `repos` labels double as deterministic test uids via `Uid`'s
/// `From<String>` — see [`repo_uid`].
fn user(username: &str, is_admin: bool, repos: &[(&str, Permission)]) -> User {
    User {
        uid: Default::default(),
        username: username.to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: repos.iter().map(|(r, p)| (repo_uid(r), *p)).collect::<BTreeMap<_, _>>(),
        default_permission: None,
        password_hash: None,
        is_admin,
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
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    }
}

fn repo_uid(label: &str) -> gitquarry_core::storage::uid::Uid {
    gitquarry_core::storage::uid::Uid::from(label.to_string())
}

#[test]
fn move_repo_form_lists_users_and_groups_as_namespace_options() {
    let users = vec![user("alice", false, &[])];
    let group_names = vec!["acme".to_string()];
    let html = render_move_repo_form(
        "team/project",
        &users,
        &group_names,
        "tok",
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains(r#"<option value="alice">alice</option>"#));
    assert!(html.contains(r#"<option value="acme">acme</option>"#));
    assert!(html.contains(r#"placeholder="team/project""#));
    assert!(html.contains(r#"value="move_repo""#));
}
