//! A real `gitquarry-tracker` (its own `web::router`) on `127.0.0.1:0`, over
//! the *same* backend and cipher as the test's `AppContext` — one bucket,
//! one passphrase, exactly how core and tracker share a bucket in a real
//! deploy (`docs/tracker-binary-split-concept.md`). Not a hand-rolled fake,
//! so the tests exercise the real loopback contract.
//!
//! Tracker's own client back into core points at a port nothing listens on:
//! every fixture leaves labels and assignees empty, so tracker never calls
//! core's `/_internal/*` to validate them.

use crate::Work;
use gitquarry_core::app::{AppContext, ResolvedRepo, TrackerLink};
use gitquarry_core::domain::repo;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_tracker::core_client::CoreClient;
use gitquarry_tracker::state::{TrackerState, TrackerTarget};
use std::sync::Arc;

/// Tracker's own state (to seed data straight through
/// `gitquarry_tracker::domain::*`) and the link `AppContext::tracker` points
/// at, both over `base`'s backend and cipher.
pub async fn spawn_tracker(base: &AppContext) -> (TrackerState, TrackerLink) {
    let state = TrackerState {
        backend: Arc::clone(&base.backend),
        cipher: Arc::clone(&base.cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
    };
    // `router()` rather than `run_server`, which only adds the
    // internal-token check: webui's client sends the header either way.
    let app = gitquarry_tracker::web::router(Arc::new(state.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding the tracker listener");
    let addr = listener.local_addr().expect("reading the tracker listener address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serving the tracker");
    });
    let link = TrackerLink {
        base_url: format!("http://{addr}"),
        internal_token: "unused".to_string(),
        http: reqwest::Client::new(),
    };
    (state, link)
}

/// A public `team/proj` whose first commit holds `files`, on an instance
/// with [`spawn_tracker`]'s tracker configured.
pub struct TrackerFixture {
    _dir: tempfile::TempDir,
    pub work: Work,
    pub ctx: Arc<AppContext>,
    pub repo: ResolvedRepo,
    pub tracker: TrackerState,
    pub target: TrackerTarget,
}

pub async fn tracker_fixture(files: &[(&str, &str)]) -> TrackerFixture {
    let dir = tempfile::tempdir().expect("creating the fixture dir");
    let base = crate::ctx(dir.path());
    let (tracker, link) = spawn_tracker(&base).await;
    let ctx = Arc::new(AppContext {
        tracker: Some(link),
        ..base
    });
    let repo = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .expect("creating team/proj")
        .expect("team/proj already exists");
    let work = Work::new().await;
    work.commit(files, "initial").await;
    work.push(&repo, "refs/heads/main").await;
    let target = TrackerTarget { uid: repo.uid.clone() };
    TrackerFixture {
        _dir: dir,
        work,
        ctx,
        repo,
        tracker,
        target,
    }
}
