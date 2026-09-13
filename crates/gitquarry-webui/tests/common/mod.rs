pub mod tracker;

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::remote::Remote;
use gitquarry_core::storage::uid::Uid;
use gitquarry_tracker::state::TrackerState;
use std::sync::Arc;

/// One `AppContext`/`ResolvedRepo` pair plus a real, running
/// `gitquarry-tracker` (its own `TrackerState`, for seeding data directly
/// via `gitquarry_tracker::domain::*` the same way tests did before the
/// binary split) sharing this test's own `LocalBackend`/`Cipher` — see
/// `tracker::spawn_tracker`'s own doc comment.
pub async fn setup(root: &std::path::Path) -> (AppContext, ResolvedRepo, TrackerState) {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[17u8; SALT_LEN]).unwrap());
    let uid = Uid::from("team/proj".to_string());
    let remote = Remote::new(Arc::clone(&backend), "repos/team/proj".to_string(), Arc::clone(&cipher));
    let target = ResolvedRepo {
        uid: uid.clone(),
        remote,
    };

    let (tracker_state, link) = tracker::spawn_tracker(Arc::clone(&backend), Arc::clone(&cipher)).await;

    let ctx = AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        tracker: Some(link),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    };

    (ctx, target, tracker_state)
}

/// `gitquarry_tracker::state::TrackerTarget` for `target.uid` — what
/// seeding data directly via `gitquarry_tracker::domain::*` needs, since
/// that crate's own domain layer takes its narrower `TrackerTarget`, not
/// core's `ResolvedRepo`.
pub fn tracker_target(target: &ResolvedRepo) -> gitquarry_tracker::state::TrackerTarget {
    gitquarry_tracker::state::TrackerTarget {
        uid: target.uid.clone(),
    }
}
