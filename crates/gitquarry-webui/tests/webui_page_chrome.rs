//! The shared chrome every page is wrapped in: the mobile nav toggle, topic
//! badges, the active-tab marker, and the federation-only entries that must
//! disappear when `[federation]` isn't configured.

use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use gitquarry_test_support::Session;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path, federation: bool) -> AppContext {
    AppContext {
        // Never dialled by the pages below; only its presence changes the
        // chrome they render.
        federation: federation.then(|| gitquarry_core::app::FederationLink {
            base_url: "http://127.0.0.1:1".to_string(),
            internal_token: "unused".to_string(),
            http: reqwest::Client::new(),
        }),
        ..gitquarry_test_support::ctx(root)
    }
}

/// An admin with a password, so both the admin pages and `/_account/*`
/// open for the session `login` returns.
async fn app_with_admin(root: &std::path::Path, federation: bool) -> axum::Router {
    let ctx = Arc::new(test_ctx(root, federation));
    gitquarry_test_support::user(&ctx, "alice", true).await;
    gitquarry_webui::webui::router(ctx)
}

/// The `web-session=...` pair a real password login hands back.
async fn login(app: &axum::Router) -> String {
    Session::login(app, "alice").await.cookie
}

async fn page(app: &axum::Router, path: &str, cookie: Option<&str>) -> String {
    let page = Session::get_as(app, path, cookie).await;
    assert_eq!(page.status, StatusCode::OK, "GET {path}");
    page.body
}

async fn index_with_topics(topics: &[&str]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), false));
    let resolved = ctx.create_repo("acme/web").await.unwrap().unwrap();
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/web".to_string(),
        visibility: Visibility::Public,
        topics: topics.iter().map(|t| t.to_string()).collect(),
        ..Default::default()
    };
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();
    gitquarry_core::storage::settings::set_visible_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, true)
        .await;
    let app = gitquarry_webui::webui::router(ctx);
    let html = page(&app, "/", None).await;
    assert!(
        html.contains(r#"href="/acme/web""#),
        "the repo isn't on the index: {html}"
    );
    html
}

#[tokio::test]
async fn every_page_carries_the_mobile_nav_checkbox_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(dir.path(), false)));
    let html = page(&app, "/_license", None).await;
    assert!(html.contains(r#"input type="checkbox" id="nav-toggle" class="nav-toggle-input""#));
    assert!(html.contains(r#"label for="nav-toggle" class="nav-toggle-btn""#));
}

#[tokio::test]
async fn topic_badges_link_to_the_index_search() {
    let html = index_with_topics(&["rust"]).await;
    assert!(html.contains(r#"href="/?q=rust""#), "{html}");
    assert!(html.contains(">rust<"), "{html}");
}

/// Topics are stored as whatever the settings form accepted; the badge is
/// markup built around them, so both the link and the label must escape.
#[tokio::test]
async fn topic_badges_escape_the_topic() {
    let html = index_with_topics(&["<b>x</b>"]).await;
    assert!(!html.contains("<b>x</b>"), "topic rendered as markup");
    assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"), "{html}");
    assert!(html.contains(r#"href="/?q=%3Cb%3Ex%3C/b%3E""#), "{html}");
}

#[tokio::test]
async fn no_topics_render_no_badges() {
    let html = index_with_topics(&[]).await;
    assert!(!html.contains(r#"class="badge accent""#), "{html}");
}

#[tokio::test]
async fn the_tabs_bar_marks_only_the_active_tab_with_aria_current() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(dir.path(), false).await;
    let session = login(&app).await;
    let html = page(&app, "/_users", Some(&session)).await;
    let link_to = |href: &str| {
        let needle = format!(r#"href="{href}""#);
        let at = html.find(&needle).unwrap_or_else(|| panic!("no link to {href}"));
        let open = html[..at].rfind("<a ").unwrap();
        html[open..at + needle.len()].to_string()
    };
    let users = link_to("/_users");
    assert!(users.contains(r#"aria-current="page""#), "{users}");
    let settings = link_to("/_settings/footer");
    assert!(!settings.contains("aria-current"), "{settings}");
}

#[tokio::test]
async fn admin_tabs_drop_peers_without_federation() {
    let with_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(with_dir.path(), true).await;
    let session = login(&app).await;
    let with = page(&app, "/_users", Some(&session)).await;
    assert!(with.contains(r#"href="/_peers""#), "{with}");

    let without_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(without_dir.path(), false).await;
    let session = login(&app).await;
    let without = page(&app, "/_users", Some(&session)).await;
    assert!(!without.contains("/_peers"), "{without}");
    // The rest of the bar is untouched -- only the federation entry goes.
    assert!(without.contains(r#"href="/_users""#), "{without}");
    assert!(without.contains(r#"href="/_audit""#), "{without}");
}

#[tokio::test]
async fn account_sidebar_drops_tokens_without_federation() {
    let with_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(with_dir.path(), true).await;
    let session = login(&app).await;
    let with = page(&app, "/_account/pat", Some(&session)).await;
    assert!(with.contains(r#"href="/_account/tokens""#), "{with}");

    let without_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(without_dir.path(), false).await;
    let session = login(&app).await;
    let without = page(&app, "/_account/pat", Some(&session)).await;
    assert!(!without.contains("/_account/tokens"), "{without}");
    assert!(without.contains(r#"href="/_account/pat""#), "{without}");
}
