//! The generated avatar every account without an uploaded one gets: stable
//! per username, distinct across usernames, and decorative.

use axum::Router;
use axum::http::StatusCode;
use gitquarry_core::auth::domain::user::{self, NewUser};
use gitquarry_test_support::{Session, ctx};
use std::path::Path;
use std::sync::Arc;

/// A router over a bucket holding `usernames`, none with an uploaded
/// avatar.
async fn app_with_users(root: &Path, usernames: &[&str]) -> Router {
    let ctx = Arc::new(ctx(root));
    // Not `gitquarry_test_support::user`: a password user would switch
    // login enforcement on, and these profiles are read anonymously.
    for username in usernames {
        user::create(
            &ctx,
            NewUser {
                username,
                email: None,
                pubkey: None,
                password_plain: None,
                is_admin: false,
                repos_field: "",
                locale: gitquarry_i18n::Locale::En,
            },
        )
        .await
        .unwrap()
        .unwrap();
    }
    gitquarry_webui::webui::router(ctx)
}

/// The avatar markup heading `username`'s profile page.
async fn profile_avatar(app: &Router, username: &str) -> String {
    let page = Session::get_as(app, &format!("/_profile/{username}"), None).await;
    let (status, html) = (page.status, page.body);
    assert_eq!(status, StatusCode::OK, "{html}");
    let heading = &html[html.find("<h3>").unwrap()..];
    let start = heading
        .find("<svg")
        .unwrap_or_else(|| panic!("no identicon on the profile: {heading}"));
    let end = heading.find("</svg>").unwrap() + "</svg>".len();
    heading[start..end].to_string()
}

#[tokio::test]
async fn same_username_renders_the_same_avatar_on_every_load() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_users(dir.path(), &["alice"]).await;
    assert_eq!(profile_avatar(&app, "alice").await, profile_avatar(&app, "alice").await);

    // Computed from the username alone, so a second instance over another
    // bucket draws the same one.
    let other = tempfile::tempdir().unwrap();
    let other_app = app_with_users(other.path(), &["alice"]).await;
    assert_eq!(
        profile_avatar(&app, "alice").await,
        profile_avatar(&other_app, "alice").await
    );
}

#[tokio::test]
async fn different_usernames_render_different_avatars() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_users(dir.path(), &["alice", "bob"]).await;
    assert_ne!(profile_avatar(&app, "alice").await, profile_avatar(&app, "bob").await);
}

/// Decorative (the username is always written out beside it, so a screen
/// reader announcing the icon too would just repeat it), and carrying no
/// pixel size of its own: `.avatar` sizes it off the surrounding text, so
/// it keeps its proportion to the name under every density and text size.
#[tokio::test]
async fn is_decorative_and_sized_by_the_stylesheet() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_users(dir.path(), &["alice"]).await;
    let svg = profile_avatar(&app, "alice").await;
    assert!(svg.contains(r#"aria-hidden="true""#), "{svg}");
    assert!(svg.contains(r#"class="avatar""#), "{svg}");
    let open_tag = &svg[..svg.find('>').unwrap()];
    assert!(
        !open_tag.contains(" width="),
        "a hardcoded pixel size came back: {open_tag}"
    );
}
