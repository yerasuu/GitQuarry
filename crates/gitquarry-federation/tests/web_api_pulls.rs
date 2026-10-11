//! `/_api/pulls*` through `web::router`, against a stand-in for core's
//! `/_internal/*` that knows two repos: `alice/demo` (alice is a reporter)
//! and `bob/secret` (private, alice has no access).

use axum::body::Body;
use axum::extract::Query;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use gitquarry_federation::domain;
use gitquarry_federation::web::router;
use gitquarry_test_support::{federation_state, send};
use gitquarry_wire::Uid;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const DEMO_UID: &str = "uid-demo";
const SECRET_UID: &str = "uid-secret";

/// Counts every MR core is asked to create, so a test can assert a refused
/// request never got that far.
async fn fake_core(created: Arc<AtomicU32>) -> String {
    let app = axum::Router::new()
        .route(
            "/_internal/repos/uid-for-name",
            axum::routing::get(|q: Query<HashMap<String, String>>| async move {
                match q.get("name").map(String::as_str) {
                    Some("alice/demo") => axum::Json(serde_json::json!({ "uid": DEMO_UID })).into_response(),
                    Some("bob/secret") => axum::Json(serde_json::json!({ "uid": SECRET_UID })).into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }),
        )
        .route(
            "/_internal/repos/public/{*name}",
            axum::routing::get(|| async { StatusCode::NOT_FOUND }),
        )
        .route(
            "/_internal/authorize",
            axum::routing::post(|axum::Json(req): axum::Json<serde_json::Value>| async move {
                let ok = req["username"] == "alice" && req["repo"] == "alice/demo" && req["require"] == "reporter";
                axum::Json(serde_json::json!({ "ok": ok }))
            }),
        )
        .route(
            "/_internal/merge-requests",
            axum::routing::post(move || {
                let created = Arc::clone(&created);
                async move {
                    created.fetch_add(1, Ordering::SeqCst);
                    axum::Json(serde_json::json!({}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/_internal", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// A read-only token alice owns, scoped to both repos — so only core's own
/// permission decision stands between it and `bob/secret`.
async fn alice_token(state: &gitquarry_federation::state::FederationState) -> String {
    let repos = [DEMO_UID, SECRET_UID]
        .into_iter()
        .map(|uid| Uid::from(uid.to_string()))
        .collect();
    let (_, secret) = domain::token::create(
        state.backend.as_ref(),
        &state.cipher,
        "alice",
        "pr api",
        repos,
        false,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    secret
}

fn post_json(path: &str, secret: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(path)
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn an_unreachable_core_is_a_bad_gateway_not_an_access_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1/_internal");
    let secret = alice_token(&state).await;

    let comment = post_json(
        "/_api/pulls/comment",
        &secret,
        serde_json::json!({ "repo": "alice/demo", "number": 1, "body": "hi" }),
    );
    assert_eq!(
        send(&router(Arc::clone(&state)), comment).await.status,
        StatusCode::BAD_GATEWAY
    );

    let list = Request::get("/_api/pulls?repo=alice/demo")
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        send(&router(Arc::clone(&state)), list).await.status,
        StatusCode::BAD_GATEWAY
    );
}

#[tokio::test]
async fn an_unknown_token_is_still_unauthorized() {
    let dir = tempfile::tempdir().unwrap();
    let created = Arc::new(AtomicU32::new(0));
    let state = federation_state(dir.path(), &fake_core(Arc::clone(&created)).await);
    let comment = post_json(
        "/_api/pulls/comment",
        "token_nope",
        serde_json::json!({ "repo": "alice/demo", "number": 1, "body": "hi" }),
    );
    assert_eq!(
        send(&router(Arc::clone(&state)), comment).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_reporter_opens_a_same_repo_merge_request() {
    let dir = tempfile::tempdir().unwrap();
    let created = Arc::new(AtomicU32::new(0));
    let state = federation_state(dir.path(), &fake_core(Arc::clone(&created)).await);
    let secret = alice_token(&state).await;
    let create = post_json(
        "/_api/pulls",
        &secret,
        serde_json::json!({ "repo": "alice/demo", "source_branch": "topic", "target_branch": "main" }),
    );
    assert_eq!(send(&router(Arc::clone(&state)), create).await.status, StatusCode::OK);
    assert_eq!(created.load(Ordering::SeqCst), 1);
}
