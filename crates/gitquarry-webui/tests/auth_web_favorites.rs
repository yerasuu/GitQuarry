use gitquarry_core::auth::entities::User;
use gitquarry_webui::auth::web::favorites::*;

fn user_with_starred(starred: &[&str]) -> User {
    User {
        uid: Default::default(),
        username: "alice".to_string(),
        email: None,
        pubkeys: vec![],
        gpg_keys: Vec::new(),
        repos: Default::default(),
        default_permission: None,
        password_hash: None,
        is_admin: false,
        session_epoch: 0,
        is_federation_moderator: false,
        starred: starred
            .iter()
            .map(|s| gitquarry_core::storage::uid::Uid::from(s.to_string()))
            .collect(),
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

#[test]
fn favorites_page_lists_starred_repos_with_an_unstar_button() {
    let starred = [("acme/proj".to_string(), 0)];
    let html = render_favorites_page(
        &user_with_starred(&["acme/proj"]),
        "tok",
        None,
        &starred,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("acme/proj"));
    assert!(html.contains(r#"value="toggle_star""#));
    assert!(html.contains(r#"action="/_account/favorites""#));
    assert!(html.contains("tok"));
}

#[test]
fn favorites_visibility_checkbox_reflects_the_current_flag() {
    let private = user_with_starred(&[]);
    let html = render_favorites_page(&private, "tok", None, &[], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"name="favorites_public""#));
    assert!(!html.contains("checked"));

    let public = User {
        favorites_public: true,
        ..private
    };
    let html = render_favorites_page(&public, "tok", None, &[], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("checked"));
}

#[test]
fn favorites_page_hints_when_no_repo_is_starred() {
    let html =
        render_favorites_page(&user_with_starred(&[]), "tok", None, &[], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("none yet"));
}

#[test]
fn favorites_page_escapes_a_hostile_repo_name() {
    // maud auto-escapes every interpolated value — no `esc()` call to
    // remember per field, unlike the old format!()-based version.
    let starred = [("<script>alert(1)</script>".to_string(), 0)];
    let html = render_favorites_page(
        &user_with_starred(&["<script>alert(1)</script>"]),
        "tok",
        None,
        &starred,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("&lt;script&gt;"));
}

#[test]
fn public_favorites_page_lists_repos_with_no_edit_controls() {
    let html = render_public_favorites_page("alice", &[("acme/proj".to_string(), 0)], gitquarry_i18n::Locale::En)
        .into_string();
    assert!(html.contains("acme/proj"));
    assert!(html.contains("alice"));
    // Read-only: no unstar button, no form at all — this is someone
    // else's list, only `alice`'s own session can edit it.
    assert!(!html.contains("toggle_star"));
    assert!(!html.contains("<form"));
}

#[test]
fn public_favorites_page_hints_when_nothing_is_visible() {
    let html = render_public_favorites_page("alice", &[], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("none visible"));
}

#[test]
fn favorites_page_shows_star_count_when_nonzero() {
    let starred = [("acme/proj".to_string(), 5)];
    let html = render_favorites_page(
        &user_with_starred(&["acme/proj"]),
        "tok",
        None,
        &starred,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains('\u{2605}'));

    let unstarred = [("acme/proj".to_string(), 0)];
    let html = render_favorites_page(
        &user_with_starred(&["acme/proj"]),
        "tok",
        None,
        &unstarred,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(!html.contains('\u{2605}'));
}

#[test]
fn public_favorites_page_shows_star_count_when_nonzero() {
    let html = render_public_favorites_page("alice", &[("acme/proj".to_string(), 5)], gitquarry_i18n::Locale::En)
        .into_string();
    assert!(html.contains('\u{2605}'));
}
