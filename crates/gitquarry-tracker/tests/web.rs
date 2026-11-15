//! Tracker's loopback HTTP API, driven over a real socket the way webui's
//! client calls it.

mod common;

use common::{test_state, test_target};
use gitquarry_i18n::Locale;
use gitquarry_tracker::core_client::CoreClient;
use gitquarry_wire::Uid;
use reqwest::StatusCode;
use std::sync::Arc;

async fn serve(state: Arc<gitquarry_tracker::state::TrackerState>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, gitquarry_tracker::web::router(state))
            .await
            .unwrap();
    });
    format!("http://{addr}/_internal")
}

async fn get_status(url: &str) -> StatusCode {
    reqwest::get(url).await.unwrap().status()
}

#[tokio::test]
async fn a_uid_as_uid_new_mints_it_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let base = serve(state).await;
    let uid = test_target().uid;
    assert_eq!(get_status(&format!("{base}/issues?uid={uid}")).await, StatusCode::OK);
}

#[tokio::test]
async fn a_uid_that_is_not_32_lowercase_hex_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let base = serve(state).await;
    let upper = Uid::new().as_str().to_ascii_uppercase();
    let uid = Uid::new();
    let short = &uid.as_str()[..31];
    for uid in ["team/proj", "..", "", short, &upper, "0123456789abcdef0123456789abcdeg"] {
        let url = reqwest::Url::parse_with_params(&format!("{base}/issues"), [("uid", uid)]).unwrap();
        assert_eq!(get_status(url.as_str()).await, StatusCode::BAD_REQUEST, "uid {uid:?}");
    }
    assert_eq!(get_status(&format!("{base}/board")).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_merge_notification_with_an_invalid_repo_uid_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let base = serve(state).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/tracker/merge-request-merged"))
        .json(&serde_json::json!({ "repo_uid": "team/proj", "mr_number": 1, "title": "fix #1", "description": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

async fn post(url: &str, body: serde_json::Value) -> (StatusCode, String) {
    let resp = reqwest::Client::new().post(url).json(&body).send().await.unwrap();
    (resp.status(), resp.text().await.unwrap())
}

#[tokio::test]
async fn a_domain_refusal_keeps_its_message_under_the_matching_status() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = gitquarry_tracker::domain::issue::create(&state, &target, "Bug", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::add_comment(&state, &target, issue.number, "alice", "mine", Locale::En)
        .await
        .unwrap();
    let (comment_id, _) = gitquarry_tracker::domain::issue::list_comments(&state, &target, issue.number)
        .await
        .unwrap()
        .remove(0);
    let base = serve(state).await;
    let uid = target.uid;

    let (status, body) = post(
        &format!("{base}/issues?uid={uid}"),
        serde_json::json!({ "title": " ", "body": "", "actor": "alice" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("issue title"), "{body}");

    let (status, body) = post(&format!("{base}/issues/99/close?uid={uid}"), serde_json::json!(null)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, _) = post(
        &format!("{base}/issues/{}/comments/{comment_id}/edit?uid={uid}", issue.number),
        serde_json::json!({ "actor": "mallory", "body": "theirs" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_storage_failure_is_a_500_that_names_nothing_internal() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    gitquarry_tracker::domain::issue::create(&state, &target, "Bug", "b", "alice")
        .await
        .unwrap();
    // Same bucket, another passphrase: every read of the issue fails to decrypt.
    let other = gitquarry_tracker::state::TrackerState {
        cipher: Arc::new(s3db_crypto::Cipher::derive("another passphrase", &[13u8; s3db_crypto::SALT_LEN]).unwrap()),
        ..(*state).clone()
    };
    let base = serve(Arc::new(other)).await;
    let resp = reqwest::get(format!("{base}/issues/1?uid={}", target.uid))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(resp.text().await.unwrap(), r#"{"error":"internal error"}"#);
}

#[tokio::test]
async fn a_failed_call_to_core_is_a_502_that_names_nothing_internal() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    gitquarry_tracker::domain::issue::create(&state, &target, "Bug", "b", "alice")
        .await
        .unwrap();
    let unreachable = gitquarry_tracker::state::TrackerState {
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()).unwrap(),
        ..(*state).clone()
    };
    let base = serve(Arc::new(unreachable)).await;
    let (status, body) = post(
        &format!("{base}/issues/1/edit?uid={}", target.uid),
        serde_json::json!({ "title": "Bug", "body": "b", "labels_csv": "bug", "assignees_csv": "", "milestone": null }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body, r#"{"error":"gitquarry-core request failed"}"#);
}
