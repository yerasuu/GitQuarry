use gitquarry_core::app::AppContext;
use gitquarry_core::mirror::recent;
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;
use std::time::Duration;

/// `mirror::gc`'s marker file name, hardcoded because it's private; its mtime
/// is what the eviction sweep reads as "last queried".
const LAST_ACCESS_MARKER: &str = ".gitquarry-last-access";

#[tokio::test]
async fn recorded_repos_come_back_most_recent_first() {
    let root = tempfile::tempdir().unwrap();
    let backend = gitquarry_test_support::backend(root.path());
    let cipher = gitquarry_test_support::cipher();

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
    let backend = gitquarry_test_support::backend(root.path());
    let cipher = gitquarry_test_support::cipher();

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
    let root = tempfile::tempdir().unwrap();
    let ctx = AppContext {
        mirror_max_age: Duration::from_secs(3600),
        mirror_max_size_bytes: u64::MAX,
        ..gitquarry_test_support::ctx(root.path())
    };
    let mirrors = ctx.mirrors_root.clone();

    let uid = Uid::from("cold-start-repo".to_string());
    recent::record_use(ctx.backend().as_ref(), &ctx.cipher, &uid).await;

    gitquarry_core::mirror::gc::prewarm_recent_mirrors(Arc::new(ctx)).await;

    let mirror = mirrors.join("cold-start-repo");
    assert!(mirror.join("HEAD").is_file(), "prewarm should have created the mirror");
    assert!(
        mirror.join(LAST_ACCESS_MARKER).is_file(),
        "a mirror with no last-access marker is evicted by the very next sweep"
    );
}

/// A restart must not look like traffic: warming a mirror that's already
/// on disk leaves its last-access mtime alone, so `gc`'s least-recently-
/// queried eviction order survives one.
#[tokio::test]
async fn prewarm_does_not_refresh_an_existing_mirrors_last_access() {
    let root = tempfile::tempdir().unwrap();
    let ctx = AppContext {
        mirror_max_age: Duration::from_secs(3600),
        mirror_max_size_bytes: u64::MAX,
        ..gitquarry_test_support::ctx(root.path())
    };
    let mirrors = ctx.mirrors_root.clone();

    let uid = Uid::from("already-warm-repo".to_string());
    recent::record_use(ctx.backend().as_ref(), &ctx.cipher, &uid).await;

    let mirror = mirrors.join("already-warm-repo");
    std::fs::create_dir_all(&mirror).unwrap();
    std::fs::write(mirror.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    let marker = mirror.join(LAST_ACCESS_MARKER);
    std::fs::write(&marker, b"").unwrap();
    let backdated = std::time::SystemTime::now() - Duration::from_secs(600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&marker)
        .unwrap()
        .set_modified(backdated)
        .unwrap();

    gitquarry_core::mirror::gc::prewarm_recent_mirrors(Arc::new(ctx)).await;

    assert_eq!(
        std::fs::metadata(&marker).unwrap().modified().unwrap(),
        backdated,
        "prewarming an on-disk mirror must not count as a query"
    );
}
