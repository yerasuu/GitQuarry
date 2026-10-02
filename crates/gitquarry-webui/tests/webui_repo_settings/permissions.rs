use crate::support::*;
use axum::http::StatusCode;
use gitquarry_core::auth::entities::Permission;

/// Isolates the `<tr>...</tr>` for `username` by its hidden `username`
/// field, which is unique per row regardless of the identicon rendered
/// before the visible name.
fn row_for<'a>(html: &'a str, username: &str) -> &'a str {
    let marker = format!(r#"name="username" value="{username}""#);
    html.split("<tr>")
        .find(|row| row.contains(&marker))
        .unwrap_or_else(|| panic!("no row for {username} in {html}"))
}

#[tokio::test]
async fn perm_grid_checks_the_radio_matching_the_explicit_entry() {
    let gq = Instance::new().await;
    gq.user_with_perm("bob", Permission::Developer).await;
    gq.user_with_perm("carol", Permission::Reporter).await;
    gq.user("dave", true).await;
    gq.grant("dave", Permission::Reporter).await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_permissions"), Some(&root)).await.body;

    let bob = row_for(&html, "bob");
    assert!(bob.contains(r#"value="developer" checked"#), "{bob}");
    assert!(!bob.contains(r#"value="reporter" checked"#), "{bob}");
    assert!(!bob.contains(r#"value="none" checked"#), "{bob}");

    let carol = row_for(&html, "carol");
    assert!(carol.contains(r#"value="reporter" checked"#), "{carol}");

    let dave = row_for(&html, "dave");
    assert!(dave.contains("badge accent"), "admin badge missing: {dave}");
    assert!(!dave.contains(r#"value="none" checked"#), "{dave}");
}

#[tokio::test]
async fn perm_grid_says_so_without_granted_users() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_permissions"), Some(&root)).await.body;
    assert!(html.contains("no explicit permissions"), "{html}");
}

/// The repo's namespace owner grants a tier without being an admin, and
/// the grid reflects it.
#[tokio::test]
async fn the_owner_can_grant_a_tier_from_the_grid() {
    let gq = Instance::new().await;
    gq.user("bob", false).await;
    let alice = gq.login("alice").await;
    let saved = gq
        .save(
            &alice,
            &[("conf_action", "set_perm"), ("username", "bob"), ("perm", "maintainer")],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    assert!(
        row_for(&saved.body, "bob").contains(r#"value="maintainer" checked"#),
        "{}",
        saved.body
    );
}
