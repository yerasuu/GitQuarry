use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_webui::webui::repo_settings::permissions::*;
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

/// Isolates the `<tr>...</tr>` for `username` — matches the hidden
/// `name="username"` field's `value="..."` rather than the visible
/// name text (which now sits after an identicon `<svg>`, not right
/// after `<td>`), since that hidden value is unique per row regardless
/// of what's rendered before the visible username in the same cell.
fn row_for<'a>(html: &'a str, username: &str) -> &'a str {
    let marker = format!(r#"value="{username}""#);
    html.split("<tr>").find(|row| row.contains(&marker)).unwrap()
}

#[test]
fn perm_grid_checks_the_radio_matching_the_explicit_entry() {
    let alice = user("alice", false, &[("proyectoA", Permission::Developer)]);
    let bob = user("bob", false, &[("proyectoA", Permission::Reporter)]);
    let carol = user("carol", true, &[("proyectoA", Permission::Reporter)]);
    let granted = vec![
        (alice, Permission::Developer),
        (bob, Permission::Reporter),
        (carol, Permission::Reporter),
    ];
    let html = render_perm_grid("proyectoA", &granted, "tok", gitquarry_i18n::Locale::En).into_string();

    let alice_row = row_for(&html, "alice");
    assert!(alice_row.contains(r#"value="developer" checked"#), "{alice_row}");
    assert!(!alice_row.contains(r#"value="reporter" checked"#));
    assert!(!alice_row.contains(r#"value="none" checked"#));

    let bob_row = row_for(&html, "bob");
    assert!(bob_row.contains(r#"value="reporter" checked"#), "{bob_row}");

    let carol_row = row_for(&html, "carol");
    assert!(carol_row.contains("badge accent")); // admin badge
    assert!(!carol_row.contains(r#"value="none" checked"#)); // never pre-checked
}

#[test]
fn perm_grid_empty_without_granted_users() {
    let html = render_perm_grid("proyectoA", &[], "tok", gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("no explicit permissions"));
}
