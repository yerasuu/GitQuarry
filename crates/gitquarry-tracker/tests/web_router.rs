//! `web::router` driven in-process with `tower::ServiceExt::oneshot`: the
//! status each domain outcome maps to, and the core -> tracker merge push.
//! `web.rs` covers the same mapping over a socket for issues; these cover
//! the board and milestone routes and the merge notification.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::{test_state, test_target};
use gitquarry_i18n::Locale;
use gitquarry_tracker::domain::issue;
use gitquarry_tracker::state::TrackerState;
use gitquarry_tracker::storage::issues::IssueStatus;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

async fn call(
    state: &Arc<TrackerState>,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            req = req.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let resp = gitquarry_tracker::web::router(Arc::clone(state))
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn an_invalid_milestone_is_a_400_with_its_message() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let uid = test_target().uid;
    let (status, body) = call(
        &state,
        Method::POST,
        &format!("/_internal/milestones?uid={uid}"),
        Some(serde_json::json!({ "title": " ", "description": "", "due_date": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("error"), "{body}");
}

#[tokio::test]
async fn a_missing_milestone_is_a_404() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let uid = test_target().uid;
    let (status, _) = call(
        &state,
        Method::POST,
        &format!("/_internal/milestones/42/close?uid={uid}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_missing_board_column_is_a_404() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let uid = test_target().uid;
    let (status, _) = call(
        &state,
        Method::POST,
        &format!("/_internal/board/columns/9/remove?uid={uid}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn editing_a_comment_that_is_not_yours_is_a_403() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let created = issue::create(&state, &target, "Bug", "b", "alice").await.unwrap();
    issue::add_comment(&state, &target, created.number, "alice", "mine", Locale::En)
        .await
        .unwrap();
    let (comment_id, _) = issue::list_comments(&state, &target, created.number)
        .await
        .unwrap()
        .remove(0);
    let uid = target.uid;
    let (status, _) = call(
        &state,
        Method::POST,
        &format!(
            "/_internal/issues/{}/comments/{comment_id}/edit?uid={uid}",
            created.number
        ),
        Some(serde_json::json!({ "actor": "mallory", "body": "theirs" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_request_without_uid_is_a_400() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let (status, body) = call(&state, Method::GET, "/_internal/milestones", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("missing ?uid="), "{body}");
}

#[tokio::test]
async fn an_unknown_route_is_a_404() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let (status, _) = call(&state, Method::GET, "/_internal/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn merge_notification(state: &Arc<TrackerState>, uid: &str, title: &str, description: &str) -> StatusCode {
    call(
        state,
        Method::POST,
        "/_internal/tracker/merge-request-merged",
        Some(serde_json::json!({ "repo_uid": uid, "mr_number": 7, "title": title, "description": description })),
    )
    .await
    .0
}

#[tokio::test]
async fn a_merge_closes_the_issues_its_title_and_description_reference() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    for title in ["one", "two", "three"] {
        issue::create(&state, &target, title, "", "alice").await.unwrap();
    }
    let status = merge_notification(&state, target.uid.as_str(), "Fixes #1", "also closes #3").await;
    assert_eq!(status, StatusCode::OK);
    let open = |n| {
        let (state, target) = (Arc::clone(&state), target.clone());
        async move { issue::get(&state, &target, n).await.unwrap().unwrap().status }
    };
    assert_eq!(open(1).await, IssueStatus::Closed);
    assert_eq!(open(2).await, IssueStatus::Open);
    assert_eq!(open(3).await, IssueStatus::Closed);
}

#[tokio::test]
async fn a_merge_referencing_a_missing_issue_still_answers_200() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let status = merge_notification(&state, target.uid.as_str(), "Fixes #99", "").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_malformed_merge_body_is_rejected_before_the_handler() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let (status, _) = call(
        &state,
        Method::POST,
        "/_internal/tracker/merge-request-merged",
        Some(serde_json::json!({ "title": "Fixes #1" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
