//! A repo's `?view=settings*` pages and the one `POST /{repo}` every
//! sub-form shares: the sidebar/section dispatch, and the gates in front of
//! both — admin-only GET, CSRF plus per-repo tier on the POST.

use crate::support::*;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::auth::entities::Permission;

/// Regression: 13 sections wrapped badly (or forced `table.tabs`'s own
/// horizontal scroll) in the old horizontal `project_subnav` row — the
/// sidebar reuses the account pages' `.settings-layout`/`.settings-sidebar`/
/// `.settings-content` classes instead, so losing any of them quietly
/// brings back the old overflow.
#[tokio::test]
async fn sections_render_as_a_sidebar_not_a_horizontal_row() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let page = gq.get(&settings_url("settings_labels"), Some(&root)).await;
    assert_eq!(page.status, StatusCode::OK);
    let html = &page.body;
    assert!(html.contains(r#"<div class="settings-layout">"#), "{html}");
    assert!(html.contains(r#"<nav class="settings-sidebar""#), "{html}");
    assert!(html.contains(r#"<div class="settings-content">"#), "{html}");
    assert!(!html.contains(r#"<table class="tabs subnav">"#), "{html}");
    assert!(
        html.contains(r#"<a class="active" aria-current="page" href="/alice/proj?view=settings_labels">labels</a>"#),
        "{html}"
    );
}

#[tokio::test]
async fn sub_nav_lists_every_section_with_its_own_label() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings"), Some(&root)).await.body;
    for label in [
        "repo settings",
        "default branch",
        "protected branches",
        "protected tags",
        "signed commits",
        "topics",
        "required reviewers",
        "labels",
        "size limit (LFS)",
        "per-user permissions",
        "external mirrors",
        "move repo",
        "delete repo",
    ] {
        assert!(html.contains(label), "sub-nav missing {label:?} in {html}");
    }
}

#[tokio::test]
async fn each_section_shows_only_its_own_form() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;

    let general = gq.get(&settings_url("settings"), Some(&root)).await.body;
    assert!(general.contains(r#"name="description""#), "{general}");
    assert!(!general.contains(r#"name="labels""#), "{general}");
    assert!(!general.contains(r#"name="new_local_name""#), "{general}");

    let labels = gq.get(&settings_url("settings_labels"), Some(&root)).await.body;
    assert!(labels.contains(r#"name="labels""#), "{labels}");
    assert!(!labels.contains(r#"name="description""#), "{labels}");
    assert!(!labels.contains(r#"name="new_local_name""#), "{labels}");

    let danger = gq.get(&settings_url("settings_danger_zone"), Some(&root)).await.body;
    assert!(danger.contains(r#"name="confirm_name""#), "{danger}");
    assert!(!danger.contains(r#"name="description""#), "{danger}");
    assert!(!danger.contains(r#"name="labels""#), "{danger}");
}

/// A stale bookmark or typo'd `?view=` lands on the general form with the
/// general tab highlighted, rather than content with nothing marked active.
#[tokio::test]
async fn unrecognized_section_falls_back_to_general() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_nonexistent"), Some(&root)).await.body;
    assert!(html.contains(r#"name="description""#), "{html}");
    assert!(
        html.contains(r#"<a class="active" aria-current="page" href="/alice/proj?view=settings">repo settings</a>"#),
        "{html}"
    );
}

/// Regression: the general/topics/labels sub-forms all target
/// `#repo-settings-body` — losing that quietly brings back the browser's
/// "confirm resubmission" dialog on F5. `settings_move`/`settings_danger_zone`
/// are deliberately excluded: their success path is a real redirect, and
/// `fetch()` follows it transparently, which would swap the redirect
/// target's markup into `#repo-settings-body`.
#[tokio::test]
async fn every_section_wraps_itself_and_only_the_redirecting_ones_skip_async() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    for section in ["settings", "settings_topics", "settings_labels"] {
        let html = gq.get(&settings_url(section), Some(&root)).await.body;
        assert!(html.contains(r#"<div id="repo-settings-body">"#), "{section}: {html}");
        assert!(
            html.contains(r##"data-async data-async-target="#repo-settings-body""##),
            "{section}: {html}"
        );
    }
    for section in ["settings_move", "settings_danger_zone"] {
        let html = gq.get(&settings_url(section), Some(&root)).await.body;
        assert!(
            !html.contains(r##"data-async-target="#repo-settings-body""##),
            "{section}: {html}"
        );
    }
}

/// An async save answers with just the re-rendered section, wrapped in the
/// id its form targets, instead of a whole page.
#[tokio::test]
async fn an_async_save_answers_with_the_section_fragment() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let fields = [
        ("csrf", root.csrf.as_str()),
        ("conf_action", "set_topics"),
        ("topics", "rust"),
    ];
    let request = Request::post(format!("/{REPO}"))
        .header(header::COOKIE, &root.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(&fields)
                .finish(),
        ))
        .unwrap();
    let page = gitquarry_test_support::send(&gq.app, request).await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.body;
    assert!(html.starts_with(r#"<div id="repo-settings-body">"#), "{html}");
    assert!(!html.contains("<html"), "{html}");
}

#[tokio::test]
async fn an_anonymous_visit_is_sent_to_login() {
    let gq = Instance::new().await;
    let page = gq.get(&settings_url("settings"), None).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    // Back to this very page once logged in: `?`/`=` percent-encoded, `/` kept.
    assert_eq!(page.location(), format!("/_login?return_to=/{REPO}%3Fview%3Dsettings"));
    assert!(!page.body.contains(r#"name="description""#));
}

/// The GET is the admin surface: even the repo's own namespace owner can't
/// open it, since it's also exempt from the private-repo visibility check.
#[tokio::test]
async fn only_an_admin_can_open_the_settings_page() {
    let gq = Instance::new().await;
    let alice = gq.login("alice").await;
    let page = gq.get(&settings_url("settings"), Some(&alice)).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert!(!page.body.contains(r#"name="description""#), "{}", page.body);
}

#[tokio::test]
async fn a_post_without_the_sessions_csrf_token_changes_nothing() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    for csrf in ["", "not-the-token"] {
        let forged = Session {
            cookie: root.cookie.clone(),
            csrf: csrf.to_string(),
        };
        let page = gq
            .save(&forged, &[("conf_action", "set_topics"), ("topics", "rust")])
            .await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "csrf {csrf:?}");
    }
    // Another session's token doesn't validate under this one.
    let alice = gq.login("alice").await;
    let crossed = Session {
        cookie: root.cookie,
        csrf: alice.csrf,
    };
    let page = gq
        .save(&crossed, &[("conf_action", "set_topics"), ("topics", "rust")])
        .await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST);
    assert!(gq.config().await.topics.is_empty());
}

#[tokio::test]
async fn a_developer_cannot_change_settings() {
    let gq = Instance::new().await;
    gq.user_with_perm("dev", Permission::Developer).await;
    let dev = gq.login("dev").await;
    let page = gq
        .save(&dev, &[("conf_action", "set_topics"), ("topics", "rust")])
        .await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert!(gq.config().await.topics.is_empty());
}

/// A `Maintainer` grant lets everyday settings through without `is_admin`.
#[tokio::test]
async fn a_maintainer_can_change_everyday_settings() {
    let gq = Instance::new().await;
    gq.user_with_perm("mae", Permission::Maintainer).await;
    let mae = gq.login("mae").await;
    let page = gq
        .save(&mae, &[("conf_action", "set_topics"), ("topics", "rust")])
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(gq.config().await.topics.contains("rust"));
}

/// Delete/move/mirrors and granting `Maintainer`+ need `Owner` tier.
#[tokio::test]
async fn a_maintainer_cannot_take_owner_tier_actions() {
    let gq = Instance::new().await;
    gq.user_with_perm("mae", Permission::Maintainer).await;
    gq.user("bob", false).await;
    let mae = gq.login("mae").await;
    let attempts: [&[(&str, &str)]; 4] = [
        &[("conf_action", "delete_repo"), ("confirm_name", REPO)],
        &[
            ("conf_action", "move_repo"),
            ("new_namespace", "bob"),
            ("new_local_name", "proj"),
            ("confirm_name", REPO),
        ],
        &[
            ("conf_action", "add_mirror"),
            ("remote_url", "https://github.com/acme/demo.git"),
            ("token", "ghp_secret"),
        ],
        &[("conf_action", "set_perm"), ("username", "bob"), ("perm", "maintainer")],
    ];
    for fields in attempts {
        let page = gq.save(&mae, fields).await;
        assert_eq!(page.status, StatusCode::FORBIDDEN, "{fields:?}");
    }
    assert!(gq.config().await.deleted_at.is_none());
    assert!(gq.ctx.resolve_repo(REPO).await.unwrap().is_some());
}

/// Skipping the deletion grace window is an instance-admin call, beyond
/// what even the repo's own `Owner` tier grants.
#[tokio::test]
async fn only_an_admin_can_purge() {
    let gq = Instance::new().await;
    let alice = gq.login("alice").await;
    let deleted = gq
        .save(&alice, &[("conf_action", "delete_repo"), ("confirm_name", REPO)])
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER, "{}", deleted.body);
    let purge = gq
        .save(&alice, &[("conf_action", "purge_repo"), ("confirm_name", REPO)])
        .await;
    assert_eq!(purge.status, StatusCode::FORBIDDEN);
    assert!(gq.ctx.resolve_repo(REPO).await.unwrap().is_some());
}
