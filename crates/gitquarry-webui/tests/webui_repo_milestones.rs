#![cfg(feature = "tracker")]
//! The milestones view and the milestone picker on an issue, driven through
//! `webui::router` against a real tracker.

use axum::http::StatusCode;
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::Permission;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Session, TrackerFixture, tracker_fixture};
use std::sync::Arc;

async fn view(f: &TrackerFixture, query: &str, cookie: Option<&str>) -> String {
    let app = gitquarry_webui::webui::router(Arc::clone(&f.ctx));
    let page = Session::get_as(&app, &format!("/team/proj?{query}"), cookie).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    page.body
}

#[tokio::test]
async fn milestones_list_shows_hint_when_empty() {
    let f = tracker_fixture(&[]).await;
    let html = view(&f, "view=milestones", None).await;
    assert!(html.contains("no open milestones"), "{html}");
    assert!(html.contains("no closed milestones"), "{html}");
}

#[tokio::test]
async fn milestones_list_shows_each_milestone_split_by_status() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::milestone::create(&f.tracker, &f.target, "v1.0", "first stable", "2026-12-01")
        .await
        .unwrap();
    let closed = gitquarry_tracker::domain::milestone::create(&f.tracker, &f.target, "v0.9", "beta", "")
        .await
        .unwrap();
    gitquarry_tracker::domain::milestone::close(&f.tracker, &f.target, closed.number, Locale::En)
        .await
        .unwrap();

    let html = view(&f, "view=milestones", None).await;
    assert!(html.contains("v1.0"), "{html}");
    assert!(html.contains("v0.9"), "{html}");
    assert!(html.contains("2026-12-01"), "{html}");
    // Anonymous viewer: no manage forms (create/close/reopen).
    assert!(!html.contains(r#"action="/_milestones""#));
    assert!(!html.contains(r#"action="/_milestones/close""#));
}

#[tokio::test]
async fn issue_edit_form_marks_the_current_milestone_selected() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::milestone::create(&f.tracker, &f.target, "v1.0", "", "")
        .await
        .unwrap();
    let current = gitquarry_tracker::domain::milestone::create(&f.tracker, &f.target, "v2.0", "", "")
        .await
        .unwrap();
    let issue = gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Bug", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::edit(
        &f.tracker,
        &f.target,
        issue.number,
        gitquarry_tracker::domain::issue::IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: Some(current.number),
        },
        Locale::En,
    )
    .await
    .unwrap();
    gitquarry_test_support::user(&f.ctx, "carol", false).await;
    user::set_repo_perm(&f.ctx, "carol", "team/proj", Some(Permission::Developer), Locale::En)
        .await
        .unwrap();
    let app = gitquarry_webui::webui::router(Arc::clone(&f.ctx));
    let session = Session::login(&app, "carol").await;

    let html = view(
        &f,
        &format!("view=issue&number={}", issue.number),
        Some(&session.cookie),
    )
    .await;
    assert!(
        html.contains(&format!(r#"value="{}" selected"#, current.number)),
        "{html}"
    );
    assert!(!html.contains(r#"value="1" selected"#), "{html}");
}
