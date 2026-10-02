//! `/_account/favorites` (the owner's editable starred list) and
//! `/_profile/<user>/favorites` (everyone else's read-only view), driven
//! through `router()` with real sessions.

use gitquarry_core::storage::entities::Visibility;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_test_support::{Page, Session, ctx, send, user};
use std::sync::Arc;

/// The page at `uri` for `session`, which must render.
async fn page(app: &axum::Router, session: &Session, uri: &str) -> String {
    let page = session.get(app, uri).await;
    assert_eq!(page.status, StatusCode::OK, "GET {uri}");
    page.body
}

async fn app_with_users(root: &std::path::Path) -> (Arc<AppContext>, axum::Router, Session) {
    let ctx = ctx(root);
    user(&ctx, "alice", false).await;
    user(&ctx, "bob", false).await;
    gitquarry_core::domain::repo::create(&ctx, "acme/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let ctx = Arc::new(ctx);
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let session = Session::login(&app, "alice").await;
    (ctx, app, session)
}

/// Rewrites `repo`'s stored config in place — the display name, visibility
/// and star count a repo page's own settings would otherwise set.
async fn edit_repo(
    ctx: &AppContext,
    repo: &str,
    edit: impl FnOnce(&mut gitquarry_core::storage::entities::RepoConfig),
) {
    let resolved = ctx.resolve_repo(repo).await.unwrap().unwrap();
    let (mut config, etag) = resolved.remote.get_repo_config().await.unwrap();
    edit(&mut config);
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();
}

async fn star(app: &axum::Router, session: &Session, repo: &str, return_to: &str) -> Page {
    let fields = [("star_action", "toggle_star"), ("repo", repo), ("return_to", return_to)];
    session.post(app, "/_account/favorites", &fields).await
}

async fn set_public(app: &axum::Router, session: &Session) {
    let fields = [("favorites_action", "set_visibility"), ("favorites_public", "1")];
    let response = session.post(app, "/_account/favorites", &fields).await;
    assert_eq!(response.status, StatusCode::OK);
}

#[tokio::test]
async fn favorites_page_lists_starred_repos_with_an_unstar_button() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;
    assert_eq!(star(&app, &session, "acme/proj", "").await.status, StatusCode::OK);

    let html = page(&app, &session, "/_account/favorites").await;
    assert!(html.contains(r#"href="/acme/proj""#), "{html}");
    assert!(html.contains(r#"value="toggle_star""#));
    assert!(html.contains(r#"action="/_account/favorites""#));
    // Regression: the row's "remove" form carries `favorites_action=
    // remove_from_list` alongside `star_action=toggle_star` — that's what
    // tells the handler to send back the whole `#favorites-body` (the row
    // just disappeared) instead of the bare star-button fragment a star
    // button elsewhere on the site expects. Losing either attribute here
    // means either a broken async swap or the browser's "confirm
    // resubmission" dialog coming back on F5.
    assert!(html.contains(r#"value="remove_from_list""#), "{html}");
    assert_eq!(
        html.matches(r##"data-async data-async-target="#favorites-body""##)
            .count(),
        2,
        "{html}"
    );

    let fields = [
        ("csrf", session.csrf.as_str()),
        ("star_action", "toggle_star"),
        ("favorites_action", "remove_from_list"),
        ("repo", "acme/proj"),
    ];
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    // `style/app.js` marks a `data-async` submit this way, which gets back
    // just the fragment its target names.
    let request = Request::post("/_account/favorites")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(body))
        .unwrap();
    let fragment = send(&app, request).await.body;
    assert!(fragment.starts_with(r#"<div id="favorites-body">"#), "{fragment}");
    assert!(fragment.contains("none yet"), "{fragment}");
}

#[tokio::test]
async fn favorites_page_hints_when_no_repo_is_starred() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;
    let html = page(&app, &session, "/_account/favorites").await;
    assert!(html.contains("none yet"), "{html}");
}

#[tokio::test]
async fn favorites_visibility_checkbox_reflects_the_current_flag() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;

    let html = page(&app, &session, "/_account/favorites").await;
    assert!(
        html.contains(r#"<input type="checkbox" name="favorites_public" value="1">"#),
        "{html}"
    );

    set_public(&app, &session).await;
    let html = page(&app, &session, "/_account/favorites").await;
    assert!(
        html.contains(r#"<input type="checkbox" name="favorites_public" value="1" checked>"#),
        "{html}"
    );
}

#[tokio::test]
async fn favorites_row_shows_the_display_name_and_links_the_slug() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    edit_repo(&ctx, "acme/proj", |c| c.display_name = Some("My Proj!".to_string())).await;
    star(&app, &session, "acme/proj", "").await;

    let html = page(&app, &session, "/_account/favorites").await;
    assert!(html.contains(r#"href="/acme/proj""#), "{html}");
    assert!(html.contains("acme/My Proj!"), "{html}");
    // The unstar form still carries the slug: that's what the handler
    // resolves, a display name resolves to nothing.
    assert!(html.contains(r#"name="repo" value="acme/proj""#), "{html}");
}

/// The display name is free text, unlike the slug — so it is the field a
/// hostile repo owner controls, on a page other accounts render.
#[tokio::test]
async fn favorites_pages_escape_a_hostile_display_name() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    edit_repo(&ctx, "acme/proj", |c| {
        c.display_name = Some("<script>alert(1)</script>".to_string())
    })
    .await;
    star(&app, &session, "acme/proj", "").await;
    set_public(&app, &session).await;

    let own = page(&app, &session, "/_account/favorites").await;
    let bob = Session::login(&app, "bob").await;
    let public = bob.get(&app, "/_profile/alice/favorites").await.body;
    for html in [own, public] {
        assert!(!html.contains("<script>alert"), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{html}");
    }
}

#[tokio::test]
async fn favorites_page_shows_star_count_when_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    star(&app, &session, "acme/proj", "").await;
    set_public(&app, &session).await;

    let html = page(&app, &session, "/_account/favorites").await;
    assert!(html.contains("1 \u{2605}"), "{html}");
    let public = Session::get_as(&app, "/_profile/alice/favorites", None).await.body;
    assert!(public.contains("1 \u{2605}"), "{public}");

    edit_repo(&ctx, "acme/proj", |c| c.star_count = 0).await;
    let html = page(&app, &session, "/_account/favorites").await;
    assert!(!html.contains('\u{2605}'), "{html}");
}

#[tokio::test]
async fn public_favorites_page_lists_repos_with_no_edit_controls() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;
    star(&app, &session, "acme/proj", "").await;

    // Opt-in: nobody else sees the list until its owner makes it public.
    let response = Session::get_as(&app, "/_profile/alice/favorites", None).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let response = Session::get_as(&app, "/_profile/nobody/favorites", None).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    set_public(&app, &session).await;
    let bob = Session::login(&app, "bob").await;
    let response = bob.get(&app, "/_profile/alice/favorites").await;
    assert_eq!(response.status, StatusCode::OK);
    let html = response.body;
    assert!(html.contains(r#"href="/acme/proj""#), "{html}");
    assert!(html.contains("<strong>alice</strong>"), "{html}");
    // Read-only: this is someone else's list, only alice's own session can
    // edit it.
    assert!(!html.contains("toggle_star"), "{html}");
    assert!(!html.contains("remove_from_list"), "{html}");
    assert!(!html.contains(r#"action="/_account/favorites""#), "{html}");

    // The owner's own session is sent to the editable page instead.
    let response = session.get(&app, "/_profile/alice/favorites").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/_account/favorites");
}

/// A private repo on a public favorites list must not leak to a viewer
/// with no grant on it, even though its owner's own list still shows it.
#[tokio::test]
async fn public_favorites_page_hides_what_the_viewer_cannot_see() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    star(&app, &session, "acme/proj", "").await;
    set_public(&app, &session).await;
    edit_repo(&ctx, "acme/proj", |c| c.visibility = Visibility::Private).await;

    let own = page(&app, &session, "/_account/favorites").await;
    assert!(own.contains(r#"href="/acme/proj""#), "{own}");
    let bob = Session::login(&app, "bob").await;
    let html = bob.get(&app, "/_profile/alice/favorites").await.body;
    assert!(!html.contains("acme/proj"), "{html}");
    assert!(html.contains("none visible"), "{html}");
}

/// Starring would otherwise confirm a private repo exists to anyone who
/// can guess its name.
#[tokio::test]
async fn starring_a_repo_the_viewer_cannot_see_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    edit_repo(&ctx, "acme/proj", |c| c.visibility = Visibility::Private).await;

    let response = star(&app, &session, "acme/proj", "").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let html = page(&app, &session, "/_account/favorites").await;
    assert!(html.contains("none yet"), "{html}");
}

#[tokio::test]
async fn toggling_a_star_only_redirects_to_a_local_return_to() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;

    let response = star(&app, &session, "acme/proj", "/acme/proj").await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/acme/proj");

    for hostile in ["//evil.com", "https://evil.com", "/\\evil.com"] {
        let response = star(&app, &session, "acme/proj", hostile).await;
        assert_eq!(response.status, StatusCode::OK, "{hostile}");
        assert!(response.headers.get(header::LOCATION).is_none(), "{hostile}");
    }
}

#[tokio::test]
async fn a_favorites_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("star_action", "toggle_star"),
        ("repo", "acme/proj"),
    ];
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let request = Request::post("/_account/favorites")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let html = page(&app, &session, "/_account/favorites").await;
    assert!(html.contains("none yet"), "{html}");
}
