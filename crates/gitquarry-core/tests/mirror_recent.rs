use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::mirror::recent;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::uid::Uid;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn test_backend(root: &Path) -> (Arc<dyn Backend>, Arc<Cipher>) {
    let backend: Arc<dyn Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[7u8; SALT_LEN]).unwrap());
    (backend, cipher)
}

/// An `AppContext` good enough for `prewarm_recent_mirrors`: it reads the
/// recent-use index off `backend`/`cipher` and then warms each uid through
/// `warm_mirror`, which only needs `mirrors_root`/`exe_path`/`locks`.
fn test_ctx(bucket_root: &Path, mirrors_root: PathBuf, max_age: Duration) -> gitquarry_core::app::AppContext {
    let (backend, cipher) = test_backend(bucket_root);
    gitquarry_core::app::AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root,
        config_path: PathBuf::new(),
        exe_path: PathBuf::from("/bin/true"),
        backend,
        locks: gitquarry_core::app::Locks::default(),
        cipher,
        mirror_max_age: max_age,
        mirror_max_size_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

#[tokio::test]
async fn recorded_repos_come_back_most_recent_first() {
    let root = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend(root.path());

    let older = Uid::from("order-older".to_string());
    let newer = Uid::from("order-newer".to_string());
    recent::record_use(backend.as_ref(), &cipher, &older).await;
    recent::record_use(backend.as_ref(), &cipher, &newer).await;

    let recent = recent::recent_uids(backend.as_ref(), &cipher, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(recent, vec![newer, older]);
}

#[tokio::test]
async fn an_untouched_bucket_has_nothing_to_warm() {
    let root = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend(root.path());

    let recent = recent::recent_uids(backend.as_ref(), &cipher, Duration::from_secs(3600))
        .await
        .unwrap();
    assert!(recent.is_empty());
}

/// The regression this index exists for: the mirrors dir didn't survive
/// the restart, so listing it finds nothing — the warm set has to come
/// from the bucket, or the prewarm is a no-op in exactly the case it was
/// written for.
#[tokio::test]
async fn prewarm_creates_a_mirror_the_local_dir_never_had() {
    let bucket = tempfile::tempdir().unwrap();
    let mirrors = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend(bucket.path());

    let uid = Uid::from("cold-start-repo".to_string());
    recent::record_use(backend.as_ref(), &cipher, &uid).await;

    let ctx = test_ctx(bucket.path(), mirrors.path().to_path_buf(), Duration::from_secs(3600));
    gitquarry_core::mirror::gc::prewarm_recent_mirrors(Arc::new(ctx)).await;

    let mirror = mirrors.path().join("cold-start-repo");
    assert!(mirror.join("HEAD").is_file(), "prewarm should have created the mirror");
    assert!(
        mirror.join(gitquarry_core::mirror::gc::LAST_ACCESS_MARKER).is_file(),
        "a mirror with no last-access marker is evicted by the very next sweep"
    );
}

/// A restart must not look like traffic: warming a mirror that's already
/// on disk leaves its last-access mtime alone, so `gc`'s least-recently-
/// queried eviction order survives one.
#[tokio::test]
async fn prewarm_does_not_refresh_an_existing_mirrors_last_access() {
    let bucket = tempfile::tempdir().unwrap();
    let mirrors = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend(bucket.path());

    let uid = Uid::from("already-warm-repo".to_string());
    recent::record_use(backend.as_ref(), &cipher, &uid).await;

    let mirror = mirrors.path().join("already-warm-repo");
    std::fs::create_dir_all(&mirror).unwrap();
    std::fs::write(mirror.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    let marker = mirror.join(gitquarry_core::mirror::gc::LAST_ACCESS_MARKER);
    std::fs::write(&marker, b"").unwrap();
    let backdated = std::time::SystemTime::now() - Duration::from_secs(600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&marker)
        .unwrap()
        .set_modified(backdated)
        .unwrap();

    let ctx = test_ctx(bucket.path(), mirrors.path().to_path_buf(), Duration::from_secs(3600));
    gitquarry_core::mirror::gc::prewarm_recent_mirrors(Arc::new(ctx)).await;

    assert_eq!(
        std::fs::metadata(&marker).unwrap().modified().unwrap(),
        backdated,
        "prewarming an on-disk mirror must not count as a query"
    );
}
