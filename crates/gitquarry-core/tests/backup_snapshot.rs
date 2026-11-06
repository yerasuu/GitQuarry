use gitquarry_core::backup::inventory::Class;
use gitquarry_core::backup::report::{BackupRun, Trigger};
use gitquarry_core::backup::snapshot::{self, DEFAULT_CLASSES};
use gitquarry_core::crypto::{Cipher, CompressionStrategy};
use gitquarry_core::storage::Backend;
use gitquarry_test_support::{backend, cipher};
use s3db_crypto::SALT_LEN;
use std::sync::Arc;

/// Writes `body` sealed the way live gitquarry data is: with the object's own
/// key as AAD.
async fn put_sealed(b: &Arc<dyn Backend>, c: &Cipher, key: &str, body: &[u8]) {
    let sealed = c.seal(key, body, CompressionStrategy::None).unwrap();
    let (_, etag) = b.get(key).await.unwrap();
    b.put_cas(key, sealed, etag.as_deref()).await.unwrap();
}

async fn read_sealed(b: &Arc<dyn Backend>, c: &Cipher, key: &str) -> Option<Vec<u8>> {
    let (sealed, etag) = b.get(key).await.unwrap();
    etag?;
    Some(c.open(key, &sealed).unwrap())
}

fn fresh_run() -> BackupRun {
    BackupRun::starting("self", Trigger::Cli)
}

#[tokio::test]
async fn a_snapshot_reseals_entities_under_its_own_keys() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"alice").await;
    put_sealed(&src, &c, ".conf/global.json", b"settings").await;
    put_sealed(&src, &c, "repos/r1/.conf/config.json", b"repo config").await;

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();

    assert_eq!(manifest.objects, 3);
    assert!(manifest.complete);
    // Re-sealed under the snapshot key, so it only opens with that key as
    // AAD -- which is exactly what `read_sealed` checks.
    assert_eq!(
        read_sealed(&src, &c, ".conf/backups/snap1/.conf/users/alice.json")
            .await
            .unwrap(),
        b"alice"
    );
    assert_eq!(
        read_sealed(&src, &c, ".conf/backups/snap1/repos/r1/.conf/config.json")
            .await
            .unwrap(),
        b"repo config"
    );
}

#[tokio::test]
async fn a_snapshot_leaves_out_the_classes_it_was_not_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"alice").await;
    put_sealed(&src, &c, "repos/r1/objects/pack/pack-aa.pack", b"packdata").await;
    put_sealed(&src, &c, "repos/r1/lfs/oid1", b"lfsdata").await;

    let mut run = fresh_run();
    snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();

    assert!(
        read_sealed(&src, &c, ".conf/backups/snap1/.conf/users/alice.json")
            .await
            .is_some()
    );
    // Immutable blob data: the replica already keeps every version of it, so
    // a snapshot re-sealing it would be pure duplicated cost.
    let (_, etag) = src
        .get(".conf/backups/snap1/repos/r1/objects/pack/pack-aa.pack")
        .await
        .unwrap();
    assert!(etag.is_none());
}

#[tokio::test]
async fn a_snapshot_never_captures_another_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"alice").await;

    let mut run = fresh_run();
    snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();
    let mut run = fresh_run();
    let second = snapshot::capture(&src, &c, &src, &c, "snap2", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();

    // The one user, not the user plus snap1's copy of it.
    assert_eq!(second.objects, 1);
    let (_, etag) = src
        .get(".conf/backups/snap2/.conf/backups/snap1/.conf/users/alice.json")
        .await
        .unwrap();
    assert!(etag.is_none());
}

#[tokio::test]
async fn a_snapshot_can_target_another_bucket_with_its_own_key() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = backend(src_dir.path());
    let dest = backend(dest_dir.path());
    let src_cipher = cipher();
    // A key of its own, so the assertion below proves it was resealed.
    let dest_cipher = Arc::new(Cipher::derive("test passphrase", &[2u8; SALT_LEN]).unwrap());
    put_sealed(&src, &src_cipher, ".conf/users/alice.json", b"alice").await;

    let mut run = fresh_run();
    snapshot::capture(
        &src,
        &src_cipher,
        &dest,
        &dest_cipher,
        "snap1",
        &DEFAULT_CLASSES,
        2,
        &mut run,
    )
    .await
    .unwrap();

    // Opens under the destination's own key, not the source's: unlike a
    // replica, a snapshot doesn't need the two to share a data key.
    assert_eq!(
        read_sealed(&dest, &dest_cipher, ".conf/backups/snap1/.conf/users/alice.json")
            .await
            .unwrap(),
        b"alice"
    );
}

#[tokio::test]
async fn re_running_the_same_snapshot_id_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"alice").await;

    let mut first = fresh_run();
    let first_manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut first)
        .await
        .unwrap();
    assert_eq!(first_manifest.objects, 1);

    let mut second = fresh_run();
    let second_manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut second)
        .await
        .unwrap();
    // Already captured under this id -- counted as skipped, not re-written.
    assert_eq!(second_manifest.objects, 0);
    assert_eq!(second.classes["entities"].skipped, 1);
}

#[tokio::test]
async fn snapshots_list_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"alice").await;

    for id in ["snap1", "snap2", "snap3"] {
        let mut run = fresh_run();
        snapshot::capture(&src, &c, &src, &c, id, &DEFAULT_CLASSES, 2, &mut run)
            .await
            .unwrap();
    }

    let listed = snapshot::list(&src, &c).await.unwrap();
    let ids: Vec<String> = listed.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, vec!["snap3", "snap2", "snap1"]);
}

#[tokio::test]
async fn a_restore_writes_the_snapshot_back_over_live_data() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"before").await;

    let mut run = fresh_run();
    snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();

    put_sealed(&src, &c, ".conf/users/alice.json", b"after (a mistake)").await;
    let report = snapshot::restore(&src, &c, src.as_ref(), &c, "snap1", false)
        .await
        .unwrap();

    assert_eq!(report.restored, 1);
    assert!(report.failures.is_empty());
    assert_eq!(
        read_sealed(&src, &c, ".conf/users/alice.json").await.unwrap(),
        b"before"
    );
}

#[tokio::test]
async fn a_dry_run_restore_counts_but_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"before").await;

    let mut run = fresh_run();
    snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();
    put_sealed(&src, &c, ".conf/users/alice.json", b"after").await;

    let report = snapshot::restore(&src, &c, src.as_ref(), &c, "snap1", true)
        .await
        .unwrap();
    assert_eq!(report.restored, 1);
    assert_eq!(read_sealed(&src, &c, ".conf/users/alice.json").await.unwrap(), b"after");
}

#[tokio::test]
async fn restoring_a_snapshot_that_does_not_exist_fails_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();

    let err = snapshot::restore(&src, &c, src.as_ref(), &c, "nope", true)
        .await
        .expect_err("restoring a missing snapshot must fail");
    assert!(format!("{err:#}").contains("no snapshot"), "unexpected error: {err:#}");
}

#[tokio::test]
async fn deleting_a_snapshot_removes_its_objects_and_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, ".conf/users/alice.json", b"alice").await;
    put_sealed(&src, &c, ".conf/groups/team.json", b"team").await;

    let mut run = fresh_run();
    snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();
    assert_eq!(snapshot::list(&src, &c).await.unwrap().len(), 1);

    // Two objects plus the manifest.
    assert_eq!(snapshot::delete(&src, "snap1").await.unwrap(), 3);
    assert!(snapshot::list(&src, &c).await.unwrap().is_empty());
    // The live data is untouched.
    assert_eq!(read_sealed(&src, &c, ".conf/users/alice.json").await.unwrap(), b"alice");
}

#[tokio::test]
async fn a_snapshot_id_that_would_escape_its_prefix_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    let mut run = fresh_run();

    for bad in ["../repos", ".hidden", "with/slash", ""] {
        let err = snapshot::capture(&src, &c, &src, &c, bad, &DEFAULT_CLASSES, 2, &mut run)
            .await
            .expect_err("a snapshot id must not be able to name a key outside its own prefix");
        assert!(
            format!("{err:#}").contains("backup id"),
            "unexpected error for {bad:?}: {err:#}"
        );
    }
}

#[tokio::test]
async fn a_snapshot_can_be_asked_for_a_blob_class_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    put_sealed(&src, &c, "repos/r1/lfs/oid1", b"lfsdata").await;

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &[Class::Lfs], 2, &mut run)
        .await
        .unwrap();
    assert_eq!(manifest.objects, 1);
    assert_eq!(
        read_sealed(&src, &c, ".conf/backups/snap1/repos/r1/lfs/oid1")
            .await
            .unwrap(),
        b"lfsdata"
    );
}

#[tokio::test]
async fn the_encryption_config_is_refused_as_a_snapshot_class() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    let mut run = fresh_run();

    // It isn't sealed under the data key, so there's nothing to open and
    // re-seal -- carrying it is the replica's job.
    let err = snapshot::capture(&src, &c, &src, &c, "snap1", &[Class::Bootstrap], 2, &mut run)
        .await
        .expect_err("the encryption config must not be snapshotted");
    assert!(
        format!("{err:#}").contains("encryption config"),
        "unexpected error: {err:#}"
    );
}
