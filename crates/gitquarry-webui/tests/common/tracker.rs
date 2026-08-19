//! Shared fixture for the issues/board/milestones tests: a real
//! `gitquarry-tracker` server (its own `web::router`) bound to
//! `127.0.0.1:0`, backed by the *same* `LocalBackend`/`Cipher` the test's
//! own `AppContext` uses — same bucket, same passphrase, exactly how core
//! and tracker share one bucket in a real deploy
//! (`docs/tracker-binary-split-concept.md`). Not a hand-rolled fake:
//! tracker's own web layer is cheap to stand up directly, so tests exercise
//! the real loopback contract instead of a stand-in that could drift from
//! it.
//!
//! `core`'s own loopback (`repo_labels`/`user_exists`) is left pointed at
//! an address nothing listens on — every test seeds `labels_csv`/
//! `assignees_csv` as empty strings, so `domain::issue::edit` never
//! actually calls out to it.

use gitquarry_core::app::TrackerLink;
use gitquarry_tracker::core_client::CoreClient;
use gitquarry_tracker::state::TrackerState;
use std::sync::Arc;

/// Returns tracker's own `TrackerState` (for seeding data directly via
/// `gitquarry_tracker::domain::*`, same as before the binary split) and the
/// `TrackerLink` webui's `ctx.tracker` should point at — both backed by the
/// same `backend`/`cipher` a test's own `AppContext` was built with.
pub async fn spawn_tracker(
    backend: Arc<dyn gitquarry_core::storage::Backend>,
    cipher: Arc<gitquarry_core::crypto::Cipher>,
) -> (TrackerState, TrackerLink) {
    let state = TrackerState {
        backend,
        cipher,
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
    };
    // `router()` directly, not `run_server`'s internal-token-checking
    // middleware wrapper — `tracker_client`'s own requests already carry
    // those headers, but skipping the check keeps this fixture from
    // needing to thread a real shared secret through for no test value.
    let app = gitquarry_tracker::web::router(Arc::new(state.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let link = TrackerLink {
        base_url: format!("http://{addr}"),
        internal_token: "unused".to_string(),
        http: reqwest::Client::new(),
    };
    (state, link)
}
