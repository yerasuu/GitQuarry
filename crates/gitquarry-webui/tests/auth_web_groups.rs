//! `/_groups`: shared namespaces and their members, driven through
//! `router()` with real sessions for an admin (`carol`), a group owner
//! (`alice`) and a plain member (`bob`).

use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Permission, Team};
use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use gitquarry_test_support::{Session, ctx, send, user};
use std::sync::Arc;

/// The page at `uri` for `session`, which has to render.
async fn page(app: &axum::Router, uri: &str, session: &Session) -> String {
    let page = session.get(app, uri).await;
    assert_eq!(page.status, StatusCode::OK, "GET {uri}");
    page.body
}

async fn setup(root: &std::path::Path) -> (Arc<AppContext>, axum::Router) {
    let ctx = ctx(root);
    user(&ctx, "carol", true).await;
    user(&ctx, "alice", false).await;
    user(&ctx, "bob", false).await;
    let ctx = Arc::new(ctx);
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    (ctx, app)
}

async fn uid_of(ctx: &AppContext, username: &str) -> gitquarry_core::storage::uid::Uid {
    access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap()
        .0
        .unwrap()
        .uid
}

/// `segment` under `parent` (a full group path), created by `creator`, who
/// becomes its first `Owner`.
async fn new_group(ctx: &AppContext, segment: &str, parent: Option<&str>, creator: &str) {
    let parent = match parent {
        Some(name) => Some(
            access::get_group(ctx.backend().as_ref(), &ctx.cipher, name)
                .await
                .unwrap()
                .0
                .unwrap(),
        ),
        None => None,
    };
    let creator = uid_of(ctx, creator).await;
    domain::group::create(ctx, segment, None, creator, parent.as_ref(), gitquarry_i18n::Locale::En)
        .await
        .unwrap()
        .unwrap();
}

async fn add_member(ctx: &AppContext, group: &str, username: &str, perm: &str) {
    let (group, etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, group)
        .await
        .unwrap();
    let mut group = group.unwrap();
    domain::group::set_member(ctx, &mut group, username, perm, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    access::put_group(ctx.backend().as_ref(), &ctx.cipher, &group, etag.as_deref())
        .await
        .unwrap();
}

/// `acme`, created by the admin, with alice as `Owner` and bob as `Developer`.
async fn acme(ctx: &AppContext) {
    new_group(ctx, "acme", None, "carol").await;
    add_member(ctx, "acme", "alice", "owner").await;
    add_member(ctx, "acme", "bob", "developer").await;
}

#[tokio::test]
async fn the_page_needs_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = setup(dir.path()).await;
    let response = Session::get_as(&app, "/_groups", None).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/_login?return_to=/_groups");
}

#[tokio::test]
async fn groups_page_echoes_the_search_query_in_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = setup(dir.path()).await;
    let carol = Session::login(&app, "carol").await;
    let html = page(&app, "/_groups?q=acme", &carol).await;
    assert!(html.contains(r#"input name="q" value="acme""#), "{html}");
}

/// Creating or deleting a whole group reserves or frees a namespace, which
/// stays an operator decision even for the group's own `Owner`.
#[tokio::test]
async fn only_an_admin_gets_or_may_use_delete_group() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = setup(dir.path()).await;
    acme(&ctx).await;

    let carol = Session::login(&app, "carol").await;
    let html = page(&app, "/_groups", &carol).await;
    assert!(html.contains(r#"value="delete_group""#), "{html}");

    let alice = Session::login(&app, "alice").await;
    let html = page(&app, "/_groups", &alice).await;
    assert!(!html.contains(r#"value="delete_group""#), "{html}");
    let response = alice
        .post(&app, "/_groups", &[("group_action", "delete_group"), ("name", "acme")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(
        access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
            .await
            .unwrap()
            .0
            .is_some()
    );
}

/// A member's tier is shown to everyone in the group, but only a viewer who
/// may manage it gets the forms that change it — and the POST refuses the
/// same viewer the page withheld them from.
#[tokio::test]
async fn member_forms_only_reach_a_manager() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = setup(dir.path()).await;
    acme(&ctx).await;

    let alice = Session::login(&app, "alice").await;
    let html = page(&app, "/_groups", &alice).await;
    assert!(html.contains(r#"value="set_member""#), "{html}");
    assert!(html.contains(r#"value="remove_member""#), "{html}");

    let bob = Session::login(&app, "bob").await;
    let html = page(&app, "/_groups", &bob).await;
    assert!(html.contains("<td>bob</td>"), "{html}");
    assert!(!html.contains(r#"value="set_member""#), "{html}");
    assert!(!html.contains(r#"value="remove_member""#), "{html}");

    let fields = [
        ("group_action", "set_member"),
        ("name", "acme"),
        ("username", "bob"),
        ("perm", "owner"),
    ];
    let response = bob.post(&app, "/_groups", &fields).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let (group, _etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap();
    assert_eq!(
        group.unwrap().role_of(&uid_of(&ctx, "bob").await),
        Some(Permission::Developer)
    );
}

/// The legacy fields still grant access until the `group-members` data
/// migration runs, and a team scoped to only some repos can't be shown as a
/// group-wide role — so the page has to say so rather than render a member
/// table that looks complete but isn't.
#[tokio::test]
async fn groups_page_warns_while_legacy_teams_remain() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = setup(dir.path()).await;
    acme(&ctx).await;
    let alice = Session::login(&app, "alice").await;

    let html = page(&app, "/_groups", &alice).await;
    assert!(!html.contains("gitquarry migrate"), "{html}");

    let (group, etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
        .await
        .unwrap();
    let mut group = group.unwrap();
    group.legacy_teams = vec![Team {
        name: "frontend".to_string(),
        members: BTreeSet::from([uid_of(&ctx, "alice").await]),
        permission: Permission::Developer,
        repos: Some(BTreeSet::from([gitquarry_core::storage::uid::Uid::from(
            "acme/web".to_string(),
        )])),
    }];
    access::put_group(ctx.backend().as_ref(), &ctx.cipher, &group, etag.as_deref())
        .await
        .unwrap();
    let html = page(&app, "/_groups", &alice).await;
    assert!(html.contains("gitquarry migrate"), "{html}");
}

/// Regression: every form on a group card targets the same `#groups-list`,
/// and an async submit gets exactly that fragment back. Losing any one of
/// them quietly brings back the browser's "confirm resubmission" dialog on
/// F5 for that specific action.
#[tokio::test]
async fn every_group_form_targets_groups_list() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = setup(dir.path()).await;
    new_group(&ctx, "acme", None, "carol").await;
    let carol = Session::login(&app, "carol").await;

    let fields = [
        ("csrf", carol.csrf.as_str()),
        ("group_action", "set_description"),
        ("name", "acme"),
        ("description", "the acme group"),
    ];
    // As `style/app.js` sends a `data-async` submit, which gets back just
    // the fragment its target names.
    let request = Request::post("/_groups")
        .header(header::COOKIE, &carol.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish(),
        ))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::OK);
    let fragment = response.body;
    assert!(fragment.starts_with(r#"<div id="groups-list">"#), "{fragment}");
    assert!(fragment.contains(r#"value="the acme group""#), "{fragment}");
    // set_description, set_storage_quota, set_member (row), remove_member
    // (row), set_member (add), delete_group.
    assert_eq!(
        fragment
            .matches(r##"data-async data-async-target="#groups-list""##)
            .count(),
        6,
        "{fragment}"
    );
}

/// Membership is inherited downward, so a subgroup reached only through a
/// parent's membership — no member entry of its own — is listed too.
#[tokio::test]
async fn a_member_sees_subgroups_of_their_group() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = setup(dir.path()).await;
    acme(&ctx).await;
    new_group(&ctx, "team", Some("acme"), "carol").await;
    new_group(&ctx, "widgets", None, "carol").await;

    let bob = Session::login(&app, "bob").await;
    let html = page(&app, "/_groups", &bob).await;
    assert!(html.contains("<h4>acme</h4>"), "{html}");
    assert!(html.contains("<h4>acme/team</h4>"), "{html}");
    assert!(!html.contains("widgets"), "{html}");
}

/// The bar for "may create here" is `Owner` in the group *or* in any group
/// above it — the same chain the POST enforces, so the picker can't offer
/// less (a missing option) or more (an option every submit rejects) than
/// the server accepts. Only an admin may claim a brand-new top-level
/// namespace.
#[tokio::test]
async fn the_create_form_offers_a_parent_only_where_the_viewer_may_create() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = setup(dir.path()).await;
    acme(&ctx).await;
    new_group(&ctx, "team", Some("acme"), "carol").await;
    // A sibling whose name merely starts with the same letters, which a
    // plain `starts_with` would wrongly treat as under `acme`. Alice is only
    // a `Developer` there, so it's on her page but not in her picker.
    new_group(&ctx, "acmeworks", None, "carol").await;
    add_member(&ctx, "acmeworks", "alice", "developer").await;
    new_group(&ctx, "widgets", None, "carol").await;

    let alice = Session::login(&app, "alice").await;
    let html = page(&app, "/_groups", &alice).await;
    assert!(html.contains(r#"value="add_group""#), "{html}");
    assert!(html.contains(r#"<option value="acme">"#), "{html}");
    assert!(html.contains(r#"<option value="acme/team">"#), "{html}");
    assert!(html.contains("<h4>acmeworks</h4>"), "{html}");
    assert!(!html.contains(r#"<option value="acmeworks">"#), "{html}");
    assert!(!html.contains(r#"<option value="">"#), "{html}");
    let response = alice
        .post(&app, "/_groups", &[("group_action", "add_group"), ("name", "rogue")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let response = alice
        .post(
            &app,
            "/_groups",
            &[("group_action", "add_group"), ("parent", "acmeworks"), ("name", "x")],
        )
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let response = alice
        .post(
            &app,
            "/_groups",
            &[("group_action", "add_group"), ("parent", "acme"), ("name", "ops")],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(
        access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme/ops")
            .await
            .unwrap()
            .0
            .is_some()
    );

    // `Developer` manages nothing, so there's nowhere to create.
    let bob = Session::login(&app, "bob").await;
    let html = page(&app, "/_groups", &bob).await;
    assert!(!html.contains(r#"value="add_group""#), "{html}");

    let carol = Session::login(&app, "carol").await;
    let html = page(&app, "/_groups", &carol).await;
    for option in ["", "acme", "acme/ops", "acme/team", "acmeworks", "widgets"] {
        assert!(
            html.contains(&format!(r#"<option value="{option}">"#)),
            "{option}: {html}"
        );
    }
}

#[tokio::test]
async fn a_groups_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = setup(dir.path()).await;
    let carol = Session::login(&app, "carol").await;

    let fields = [("csrf", "forged"), ("group_action", "add_group"), ("name", "acme")];
    let request = Request::post("/_groups")
        .header(header::COOKIE, &carol.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish(),
        ))
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        access::get_group(ctx.backend().as_ref(), &ctx.cipher, "acme")
            .await
            .unwrap()
            .0
            .is_none()
    );
}
