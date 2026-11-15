//! Tracker's loopback HTTP API, driven over a real socket the way webui's
//! client calls it.

mod common;

use common::{test_state, test_target};
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
