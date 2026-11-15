//! `require_internal_token` as `run_server` layers it onto `web::router`:
//! the loopback API is closed without the token and the wire-version header.

mod common;

use common::{test_state, test_target};
use gitquarry_wire::ipc::INTERNAL_TOKEN_HEADER;
use gitquarry_wire::{WIRE_PROTOCOL_VERSION, WIRE_VERSION_HEADER};
use reqwest::StatusCode;
use std::time::Duration;

const TOKEN: &str = "s3cret-token";

async fn serve_with_token() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    // `run_server` binds its own listener, so reserve a free port and hand it over.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bind = format!("127.0.0.1:{port}");
    tokio::spawn({
        let bind = bind.clone();
        async move { gitquarry_tracker::web::run_server(&bind, state, TOKEN.to_string()).await }
    });
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(&bind).await.is_ok() {
            return (format!("http://{bind}/_internal"), dir);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("tracker listener never came up on {bind}");
}

#[tokio::test]
async fn a_request_without_the_token_is_401() {
    let (base, _dir) = serve_with_token().await;
    let resp = reqwest::get(format!("{base}/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_request_with_the_wrong_token_is_401() {
    let (base, _dir) = serve_with_token().await;
    let resp = reqwest::Client::new()
        .get(format!("{base}/health"))
        .header(INTERNAL_TOKEN_HEADER, "not-it")
        .header(WIRE_VERSION_HEADER, WIRE_PROTOCOL_VERSION)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_wire_version_mismatch_is_409() {
    let (base, _dir) = serve_with_token().await;
    let resp = reqwest::Client::new()
        .get(format!("{base}/health"))
        .header(INTERNAL_TOKEN_HEADER, TOKEN)
        .header(WIRE_VERSION_HEADER, "0")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_signed_request_reaches_the_handler() {
    let (base, _dir) = serve_with_token().await;
    let uid = test_target().uid;
    let resp = reqwest::Client::new()
        .get(format!("{base}/issues?uid={uid}"))
        .header(INTERNAL_TOKEN_HEADER, TOKEN)
        .header(WIRE_VERSION_HEADER, WIRE_PROTOCOL_VERSION)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_merge_push_is_closed_without_the_token_too() {
    let (base, _dir) = serve_with_token().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/tracker/merge-request-merged"))
        .json(&serde_json::json!({ "repo_uid": test_target().uid, "mr_number": 1, "title": "Fixes #1", "description": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
