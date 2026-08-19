//! Shared test fixtures: a `TrackerState` backed by a real `LocalBackend`
//! plus a tiny in-memory stand-in for `gitquarry-core`'s own
//! `/_internal/{repos/labels,users/exists,tracker/pull-request}` — same
//! `fake_core` pattern `gitquarry-ci`'s own `tests/graph.rs` already
//! establishes, adapted to what tracker's domain layer actually calls.

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use gitquarry_tracker::core_client::CoreClient;
use gitquarry_tracker::state::{TrackerState, TrackerTarget};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Default, Clone)]
pub struct FakeCore {
    pub labels: Arc<Mutex<HashSet<String>>>,
    pub users: Arc<Mutex<HashSet<String>>>,
    pub prs: Arc<Mutex<HashSet<u64>>>,
}

impl FakeCore {
    pub fn add_label(&self, name: &str) {
        self.labels.lock().unwrap().insert(name.to_string());
    }
    pub fn add_user(&self, username: &str) {
        self.users.lock().unwrap().insert(username.to_string());
    }
    pub fn add_pr(&self, number: u64) {
        self.prs.lock().unwrap().insert(number);
    }
}

async fn handler_labels(
    State(fake): State<FakeCore>,
    Query(_params): Query<HashMap<String, String>>,
) -> axum::Json<serde_json::Value> {
    let labels: Vec<String> = fake.labels.lock().unwrap().iter().cloned().collect();
    axum::Json(serde_json::json!({ "labels": labels }))
}

async fn handler_user_exists(
    State(fake): State<FakeCore>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::Json<serde_json::Value> {
    let username = params.get("username").cloned().unwrap_or_default();
    let exists = fake.users.lock().unwrap().contains(&username);
    axum::Json(serde_json::json!({ "exists": exists }))
}

async fn handler_pr(
    State(fake): State<FakeCore>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let number: u64 = params.get("number").and_then(|s| s.parse().ok()).unwrap_or(0);
    if fake.prs.lock().unwrap().contains(&number) {
        axum::Json(gitquarry_wire::PullRequestMeta {
            number,
            ..Default::default()
        })
        .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn handler_event(_body: axum::body::Bytes) -> StatusCode {
    StatusCode::OK
}

pub async fn fake_core() -> (CoreClient, FakeCore) {
    let fake = FakeCore::default();
    let app = Router::new()
        .route("/repos/labels", get(handler_labels))
        .route("/users/exists", get(handler_user_exists))
        .route("/tracker/pull-request", get(handler_pr))
        .route("/tracker/event", axum::routing::post(handler_event))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (CoreClient::new(format!("http://{addr}"), "unused".to_string()), fake)
}

pub fn test_backend_and_cipher(root: &std::path::Path) -> (Arc<dyn s3db::Backend>, Arc<s3db_crypto::Cipher>) {
    let backend = s3db::LocalBackend::new(&s3db::LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: s3db::LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[13u8; s3db_crypto::SALT_LEN]).unwrap();
    (Arc::new(backend), Arc::new(cipher))
}

pub async fn test_state(root: &std::path::Path) -> (Arc<TrackerState>, FakeCore) {
    let (backend, cipher) = test_backend_and_cipher(root);
    let (core, fake) = fake_core().await;
    (Arc::new(TrackerState { backend, cipher, core }), fake)
}

pub fn test_target() -> TrackerTarget {
    TrackerTarget {
        uid: gitquarry_wire::Uid::from("team/proj".to_string()),
    }
}
