#![cfg(feature = "tracker")]
//! Who may POST to the tracker routes (`/_issues/*`, `/_board/*`,
//! `/_milestones/*`), driven through `webui::router` against a real tracker.
//! `webui_repo_{issues,board,milestones}.rs` cover the views and the outsider
//! 404 on a private repo; these cover the bar for someone who can see the repo.

use axum::http::StatusCode;
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Session, TrackerFixture, tracker_fixture};
use gitquarry_tracker::domain::{board, issue, milestone};
use gitquarry_tracker::state::TrackerTarget;
use gitquarry_tracker::storage::issues::IssueStatus;
use gitquarry_tracker::storage::milestones::MilestoneStatus;
use std::sync::Arc;

/// `name` logged in, holding `perm` on `team/proj` (`None`: no grant, so only
/// what a public repo gives everyone).
async fn session_as(f: &TrackerFixture, name: &str, perm: Option<Permission>) -> (axum::Router, Session) {
    gitquarry_test_support::user(&f.ctx, name, false).await;
    if perm.is_some() {
        user::set_repo_perm(&f.ctx, name, "team/proj", perm, Locale::En)
            .await
            .unwrap();
    }
    let app = gitquarry_webui::webui::router(Arc::clone(&f.ctx));
    let session = Session::login(&app, name).await;
    (app, session)
}

fn forbidden_message() -> String {
    gitquarry_i18n::shared::repo::write_permission_required(Locale::En)
}

async fn assert_forbidden(session: &Session, app: &axum::Router, route: &str, fields: &[(&str, &str)]) {
    let page = session.post(app, route, fields).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN, "{route}: {}", page.body);
    assert!(page.body.contains(&forbidden_message()), "{route}: {}", page.body);
}

// ---- board: Developer bar, no author exemption ----

#[tokio::test]
async fn board_posts_are_forbidden_below_developer() {
    let f = tracker_fixture(&[]).await;
    board::add_column(&f.tracker, &f.target, "todo").await.unwrap();
    let (app, dave) = session_as(&f, "dave", Some(Permission::Reporter)).await;
    let (_, mallory) = session_as(&f, "mallory", None).await;

    for session in [&dave, &mallory] {
        assert_forbidden(
            session,
            &app,
            "/_board/add_column",
            &[("repo", "team/proj"), ("name", "x")],
        )
        .await;
        assert_forbidden(
            session,
            &app,
            "/_board/remove_column",
            &[("repo", "team/proj"), ("column", "0")],
        )
        .await;
        assert_forbidden(
            session,
            &app,
            "/_board/add_card",
            &[("repo", "team/proj"), ("column", "0"), ("kind", "note"), ("value", "x")],
        )
        .await;
    }
    let state =
        gitquarry_tracker::storage::board::get_board(f.tracker.backend.as_ref(), &f.tracker.cipher, &f.repo.uid)
            .await
            .unwrap()
            .0;
    assert_eq!(state.columns.len(), 1);
    assert!(state.columns[0].cards.is_empty());
}

#[tokio::test]
async fn board_posts_succeed_for_a_developer() {
    let f = tracker_fixture(&[]).await;
    let (app, carol) = session_as(&f, "carol", Some(Permission::Developer)).await;
    let page = carol
        .post(&app, "/_board/add_column", &[("repo", "team/proj"), ("name", "todo")])
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    let page = carol
        .post(
            &app,
            "/_board/add_card",
            &[("repo", "team/proj"), ("column", "0"), ("kind", "note"), ("value", "x")],
        )
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    let state =
        gitquarry_tracker::storage::board::get_board(f.tracker.backend.as_ref(), &f.tracker.cipher, &f.repo.uid)
            .await
            .unwrap()
            .0;
    assert_eq!(state.columns[0].name, "todo");
    assert_eq!(state.columns[0].cards.len(), 1);
}

// ---- milestones: Developer bar, no author exemption ----

#[tokio::test]
async fn milestone_posts_are_forbidden_below_developer() {
    let f = tracker_fixture(&[]).await;
    milestone::create(&f.tracker, &f.target, "v1", "", "").await.unwrap();
    let (app, dave) = session_as(&f, "dave", Some(Permission::Reporter)).await;
    let (_, mallory) = session_as(&f, "mallory", None).await;

    for session in [&dave, &mallory] {
        assert_forbidden(session, &app, "/_milestones", &[("repo", "team/proj"), ("title", "v2")]).await;
        assert_forbidden(
            session,
            &app,
            "/_milestones/close",
            &[("repo", "team/proj"), ("number", "1")],
        )
        .await;
    }
    let all = milestone::list(&f.tracker, &f.target).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].status, MilestoneStatus::Open);
}

#[tokio::test]
async fn milestone_posts_succeed_for_a_developer() {
    let f = tracker_fixture(&[]).await;
    let (app, carol) = session_as(&f, "carol", Some(Permission::Developer)).await;
    let page = carol
        .post(&app, "/_milestones", &[("repo", "team/proj"), ("title", "v1")])
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    let page = carol
        .post(&app, "/_milestones/close", &[("repo", "team/proj"), ("number", "1")])
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    let m = milestone::get(&f.tracker, &f.target, 1).await.unwrap().unwrap();
    assert_eq!(m.status, MilestoneStatus::Closed);
}

// ---- issues: author-or-Developer to close/reopen/edit ----

async fn issue_status(f: &TrackerFixture, number: u64) -> IssueStatus {
    issue::get(&f.tracker, &f.target, number).await.unwrap().unwrap().status
}

#[tokio::test]
async fn closing_an_issue_you_did_not_open_is_forbidden_below_developer() {
    let f = tracker_fixture(&[]).await;
    issue::create(&f.tracker, &f.target, "Bug", "b", "alice").await.unwrap();
    let (app, dave) = session_as(&f, "dave", Some(Permission::Reporter)).await;
    assert_forbidden(&dave, &app, "/_issues/close", &[("repo", "team/proj"), ("number", "1")]).await;
    assert_eq!(issue_status(&f, 1).await, IssueStatus::Open);
}

#[tokio::test]
async fn reopening_an_issue_you_did_not_open_is_forbidden_below_developer() {
    let f = tracker_fixture(&[]).await;
    issue::create(&f.tracker, &f.target, "Bug", "b", "alice").await.unwrap();
    issue::close(&f.tracker, &f.target, 1, Locale::En).await.unwrap();
    let (app, dave) = session_as(&f, "dave", None).await;
    assert_forbidden(
        &dave,
        &app,
        "/_issues/reopen",
        &[("repo", "team/proj"), ("number", "1")],
    )
    .await;
    assert_eq!(issue_status(&f, 1).await, IssueStatus::Closed);
}

#[tokio::test]
async fn editing_an_issue_you_did_not_open_is_forbidden_below_developer() {
    let f = tracker_fixture(&[]).await;
    issue::create(&f.tracker, &f.target, "Bug", "b", "alice").await.unwrap();
    let (app, dave) = session_as(&f, "dave", None).await;
    assert_forbidden(
        &dave,
        &app,
        "/_issues/edit",
        &[
            ("repo", "team/proj"),
            ("number", "1"),
            ("title", "Hijacked"),
            ("body", "b"),
        ],
    )
    .await;
    let unchanged = issue::get(&f.tracker, &f.target, 1).await.unwrap().unwrap();
    assert_eq!(unchanged.title, "Bug");
}

#[tokio::test]
async fn the_author_closes_their_own_issue_without_a_grant() {
    let f = tracker_fixture(&[]).await;
    issue::create(&f.tracker, &f.target, "Bug", "b", "alice").await.unwrap();
    let (app, alice) = session_as(&f, "alice", None).await;
    let page = alice
        .post(&app, "/_issues/close", &[("repo", "team/proj"), ("number", "1")])
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    assert_eq!(issue_status(&f, 1).await, IssueStatus::Closed);
}

#[tokio::test]
async fn a_developer_closes_an_issue_someone_else_opened() {
    let f = tracker_fixture(&[]).await;
    issue::create(&f.tracker, &f.target, "Bug", "b", "alice").await.unwrap();
    let (app, carol) = session_as(&f, "carol", Some(Permission::Developer)).await;
    let page = carol
        .post(&app, "/_issues/close", &[("repo", "team/proj"), ("number", "1")])
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    assert_eq!(issue_status(&f, 1).await, IssueStatus::Closed);
}

#[tokio::test]
async fn bulk_issue_actions_are_forbidden_to_an_author_without_developer() {
    let f = tracker_fixture(&[]).await;
    issue::create(&f.tracker, &f.target, "Bug", "b", "alice").await.unwrap();
    let (app, alice) = session_as(&f, "alice", None).await;
    assert_forbidden(
        &alice,
        &app,
        "/_issues/bulk",
        &[("repo", "team/proj"), ("numbers", "1"), ("action", "close")],
    )
    .await;
    assert_eq!(issue_status(&f, 1).await, IssueStatus::Open);
}

// ---- issue comments: edit is author-only, delete adds the Developer override ----

/// An issue by alice with one comment by alice; returns the comment id.
async fn commented(f: &TrackerFixture) -> String {
    issue::create(&f.tracker, &f.target, "Bug", "b", "alice").await.unwrap();
    issue::add_comment(&f.tracker, &f.target, 1, "alice", "mine", Locale::En)
        .await
        .unwrap();
    issue::list_comments(&f.tracker, &f.target, 1)
        .await
        .unwrap()
        .remove(0)
        .0
}

async fn comment_count(f: &TrackerFixture) -> usize {
    issue::list_comments(&f.tracker, &f.target, 1).await.unwrap().len()
}

#[tokio::test]
async fn a_comment_is_edited_only_by_its_author_even_by_a_developer() {
    let f = tracker_fixture(&[]).await;
    let id = commented(&f).await;
    let (app, carol) = session_as(&f, "carol", Some(Permission::Developer)).await;
    let page = carol
        .post(
            &app,
            "/_issues/comment/edit",
            &[
                ("repo", "team/proj"),
                ("number", "1"),
                ("comment_id", &id),
                ("body", "theirs"),
            ],
        )
        .await;
    assert_eq!(page.status, StatusCode::FORBIDDEN, "{}", page.body);
    let comments = issue::list_comments(&f.tracker, &f.target, 1).await.unwrap();
    assert_eq!(comments[0].1.body, "mine");
}

#[tokio::test]
async fn a_comment_is_not_deleted_by_a_non_author_below_developer() {
    let f = tracker_fixture(&[]).await;
    let id = commented(&f).await;
    let (app, dave) = session_as(&f, "dave", Some(Permission::Reporter)).await;
    let page = dave
        .post(
            &app,
            "/_issues/comment/delete",
            &[("repo", "team/proj"), ("number", "1"), ("comment_id", &id)],
        )
        .await;
    assert_eq!(page.status, StatusCode::FORBIDDEN, "{}", page.body);
    assert_eq!(comment_count(&f).await, 1);
}

#[tokio::test]
async fn a_developer_deletes_a_comment_they_did_not_write() {
    let f = tracker_fixture(&[]).await;
    let id = commented(&f).await;
    let (app, carol) = session_as(&f, "carol", Some(Permission::Developer)).await;
    let page = carol
        .post(
            &app,
            "/_issues/comment/delete",
            &[("repo", "team/proj"), ("number", "1"), ("comment_id", &id)],
        )
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    assert_eq!(comment_count(&f).await, 0);
}

#[tokio::test]
async fn a_moderator_of_one_repo_cannot_delete_a_comment_of_another() {
    let f = tracker_fixture(&[]).await;
    let other = gitquarry_core::domain::repo::create(&f.ctx, "team/other", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let other_target = TrackerTarget { uid: other.uid.clone() };
    issue::create(&f.tracker, &other_target, "Their bug", "b", "alice")
        .await
        .unwrap();
    issue::add_comment(&f.tracker, &other_target, 1, "alice", "theirs", Locale::En)
        .await
        .unwrap();
    let foreign_id = issue::list_comments(&f.tracker, &other_target, 1)
        .await
        .unwrap()
        .remove(0)
        .0;
    // team/proj has an issue #1 too, so the number alone resolves there.
    issue::create(&f.tracker, &f.target, "Mine", "b", "alice")
        .await
        .unwrap();

    let (app, carol) = session_as(&f, "carol", Some(Permission::Developer)).await;
    let page = carol
        .post(
            &app,
            "/_issues/comment/delete",
            &[("repo", "team/proj"), ("number", "1"), ("comment_id", &foreign_id)],
        )
        .await;
    assert!(
        !page.status.is_success() && !page.status.is_redirection(),
        "{}",
        page.body
    );
    let left = issue::list_comments(&f.tracker, &other_target, 1).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].1.body, "theirs");
}
