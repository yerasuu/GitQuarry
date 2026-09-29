//! `/_users`: the admin-only user/role page, driven through `router()` with
//! a real admin session (`carol`) — and refused to everyone else.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_test_support::{Session, ctx, post_form, send};
use std::sync::Arc;

async fn setup(root: &std::path::Path) -> (Arc<AppContext>, axum::Router, Session) {
    let ctx = Arc::new(ctx(root));
    gitquarry_test_support::user(&ctx, "carol", true).await;
    gitquarry_test_support::user(&ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let carol = Session::login(&app, "carol").await;
    (ctx, app, carol)
}

async fn user(ctx: &AppContext, username: &str) -> User {
    access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap()
        .0
        .unwrap()
}

async fn users_page(app: &axum::Router, session: &Session, uri: &str) -> String {
    let page = session.get(app, uri).await;
    assert_eq!(page.status, StatusCode::OK, "GET {uri}");
    page.body
}

#[tokio::test]
async fn only_an_admin_reaches_the_page_or_its_actions() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, _carol) = setup(dir.path()).await;

    let page = Session::get_as(&app, "/_users", None).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location(), "/_login?return_to=/_users");

    let alice = Session::login(&app, "alice").await;
    assert_eq!(alice.get(&app, "/_users").await.status, StatusCode::FORBIDDEN);
    // Alice's own session and its token, pointed at the admin action.
    let fields = [("user_action", "toggle_admin"), ("username", "alice")];
    let page = alice.post(&app, "/_users", &fields).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert!(!user(&ctx, "alice").await.is_admin);
}

#[tokio::test]
async fn a_users_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("user_action", "toggle_admin"),
        ("username", "alice"),
    ];
    assert_eq!(
        post_form(&app, "/_users", Some(&carol.cookie), &fields).await.status,
        StatusCode::BAD_REQUEST
    );
    assert!(!user(&ctx, "alice").await.is_admin);
}

#[tokio::test]
async fn users_page_echoes_the_search_query_in_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, carol) = setup(dir.path()).await;
    let html = users_page(&app, &carol, "/_users?q=ali").await;
    assert!(html.contains(r#"input name="q" value="ali""#), "{html}");
    assert!(html.contains(r#"id="user-card-alice""#), "{html}");
    assert!(!html.contains(r#"id="user-card-carol""#), "{html}");
}

#[tokio::test]
async fn a_user_card_lists_each_repo_grant_with_a_remove_button() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;
    let public = gitquarry_core::storage::entities::Visibility::Public;
    gitquarry_core::domain::repo::create(
        &ctx,
        "acme/project-a",
        Some("Project A".to_string()),
        None,
        None,
        public,
    )
    .await
    .unwrap()
    .unwrap();
    let en = gitquarry_i18n::Locale::En;
    domain::user::set_repo_perm(&ctx, "alice", "acme/project-a", Some(Permission::Developer), en)
        .await
        .unwrap();
    domain::user::set_repo_perm(&ctx, "alice", "*", Some(Permission::Reporter), en)
        .await
        .unwrap();

    let html = users_page(&app, &carol, "/_users?q=alice").await;
    // Label and `full_name`: the row shows the former, its remove form
    // submits the latter.
    assert!(html.contains("<code>acme/Project A</code>"), "{html}");
    assert!(html.contains(r#"name="repo" value="acme/project-a""#), "{html}");
    assert!(html.contains(">developer</span>"), "{html}");
    assert!(html.contains("<code>*</code>"), "{html}");
    assert!(html.contains(">reporter</span>"), "{html}");
    assert!(html.contains(r#"value="remove_repo_perm""#), "{html}");

    let html = users_page(&app, &carol, "/_users?q=carol").await;
    assert!(html.contains("(no repos yet"), "{html}");
}

/// Regression: every action whose whole effect stays inside one user's own
/// card targets that card's own `id`; `delete_user` targets `#users-list`
/// instead, since it changes which cards exist. An async submit gets back
/// exactly the fragment its form names. Losing either quietly brings back
/// the browser's "confirm resubmission" dialog on F5 for that action.
#[tokio::test]
async fn card_actions_and_delete_target_the_right_fragment() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, carol) = setup(dir.path()).await;

    // `x-gq-async` because that is how `style/app.js` sends a `data-async`
    // submit, so only the fragment its target names comes back.
    let request = Request::post("/_users")
        .header(header::COOKIE, &carol.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(format!(
            "csrf={}&user_action=set_storage_quota&username=alice&storage_quota=5G",
            carol.csrf
        )))
        .unwrap();
    let page = send(&app, request).await;
    assert_eq!(page.status, StatusCode::OK);
    let card = page.body;
    assert!(
        card.starts_with(r#"<div class="card card-pad user-card" id="user-card-alice">"#),
        "{card}"
    );
    assert_eq!(
        card.matches(r##"data-async data-async-target="#user-card-alice""##)
            .count(),
        7,
        "{card}"
    );
    assert_eq!(
        card.matches(r##"data-async data-async-target="#users-list""##).count(),
        1,
        "{card}"
    );

    let request = Request::post("/_users")
        .header(header::COOKIE, &carol.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(format!(
            "csrf={}&user_action=delete_user&username=alice",
            carol.csrf
        )))
        .unwrap();
    let page = send(&app, request).await;
    assert_eq!(page.status, StatusCode::OK);
    let list = page.body;
    assert!(list.starts_with(r#"<div id="users-list">"#), "{list}");
    assert!(!list.contains("user-card-alice"), "{list}");
}

#[tokio::test]
async fn a_user_card_offers_totp_disable_only_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;

    let html = users_page(&app, &carol, "/_users?q=alice").await;
    assert!(!html.contains(r#"value="disable_totp""#), "{html}");

    let (alice, etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let mut alice = alice.unwrap();
    alice.totp_enabled = true;
    alice.totp_secret = Some("JBSWY3DPEHPK3PXP".to_string());
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &alice, etag.as_deref())
        .await
        .unwrap();
    let html = users_page(&app, &carol, "/_users?q=alice").await;
    assert!(html.contains(r#"value="disable_totp""#), "{html}");

    carol
        .post(
            &app,
            "/_users",
            &[("user_action", "disable_totp"), ("username", "alice")],
        )
        .await;
    assert!(!user(&ctx, "alice").await.totp_enabled);
}

#[tokio::test]
async fn toggle_federation_moderator_flips_the_flag_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;
    let fields = [("user_action", "toggle_federation_moderator"), ("username", "alice")];

    let page = carol.post(&app, "/_users", &fields).await;
    assert_eq!(page.status, StatusCode::OK);
    let alice = user(&ctx, "alice").await;
    assert!(alice.is_federation_moderator);
    assert!(!alice.is_admin);

    carol.post(&app, "/_users", &fields).await;
    assert!(!user(&ctx, "alice").await.is_federation_moderator);
}
