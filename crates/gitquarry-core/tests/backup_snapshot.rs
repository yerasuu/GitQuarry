use gitquarry_core::backup::inventory::Class;
use gitquarry_core::backup::report::{BackupRun, Trigger};
use gitquarry_core::backup::snapshot::{self, DEFAULT_CLASSES};
use gitquarry_core::crypto::{Cipher, CompressionStrategy};
use gitquarry_core::storage::{Backend, list_all_keys};
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
    put_stream_sealed(&src, &c, "repos/r1/lfs/oid1", b"lfsdata").await;

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &[Class::Lfs], 2, &mut run)
        .await
        .unwrap();
    assert_eq!(manifest.objects, 1);
    assert_eq!(
        read_stream_sealed(&src, &c, ".conf/backups/snap1/repos/r1/lfs/oid1").await,
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

/// Writes `body` the way `storage::remote::seal_and_upload` stores packs, LFS
/// objects and release files (and `gitquarry-artifacts` its blobs):
/// `Cipher::seal_stream` at any size, not `Cipher::seal`.
async fn put_stream_sealed(b: &Arc<dyn Backend>, c: &Cipher, key: &str, body: &[u8]) {
    let plain = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(plain.path(), body).unwrap();
    let sealed = tempfile::NamedTempFile::new().unwrap();
    c.seal_stream(key, plain.path(), sealed.path()).await.unwrap();
    b.put_stream(key, sealed.path()).await.unwrap();
}

async fn read_stream_sealed(b: &Arc<dyn Backend>, c: &Cipher, key: &str) -> Vec<u8> {
    let sealed = tempfile::NamedTempFile::new().unwrap();
    b.get_stream(key, sealed.path()).await.unwrap();
    let plain = tempfile::NamedTempFile::new().unwrap();
    c.open_stream(key, sealed.path(), plain.path()).await.unwrap();
    std::fs::read(plain.path()).unwrap()
}

#[tokio::test]
#[ignore = "needs backup::inventory to classify artifacts/* (218a0d9b, on feat/artifacts only): \
            on develop those keys are Class::Unknown and no snapshot captures them"]
async fn artifact_blobs_and_their_package_records_survive_a_snapshot_restore() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = backend(src_dir.path());
    let dest = backend(dest_dir.path());
    let c = cipher();

    // One blob under the snapshot's 1 MiB inline threshold and one over it,
    // so both the inline and the streaming re-seal are exercised.
    let small_key = format!("artifacts/blobs/{}", "aa".repeat(32));
    let big_key = format!("artifacts/blobs/{}", "bb".repeat(32));
    let small = b"a tarball".to_vec();
    let big: Vec<u8> = (0..(3 << 20)).map(|i| (i % 251) as u8).collect();
    put_stream_sealed(&src, &c, &small_key, &small).await;
    put_stream_sealed(&src, &c, &big_key, &big).await;
    let version_key = "artifacts/pkg/npm/left-pad/versions/1.0.0";
    put_sealed(&src, &c, version_key, b"{\"version\":\"1.0.0\"}").await;

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &[Class::Lfs, Class::Entities], 2, &mut run)
        .await
        .unwrap();
    assert!(manifest.complete, "capture failures: {:?}", run.failures);
    assert_eq!(manifest.objects, 3);

    let report = snapshot::restore(&src, &c, dest.as_ref(), &c, "snap1", false)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "restore failures: {:?}", report.failures);
    assert_eq!(report.restored, 3);

    assert_eq!(read_stream_sealed(&dest, &c, &small_key).await, small);
    assert_eq!(read_stream_sealed(&dest, &c, &big_key).await, big);
    assert_eq!(
        read_sealed(&dest, &c, version_key).await.unwrap(),
        b"{\"version\":\"1.0.0\"}"
    );
}

fn patterned(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// Flips the last byte, which sits inside the final AEAD tag of either
/// envelope.
async fn corrupt(b: &Arc<dyn Backend>, key: &str) {
    let (mut sealed, etag) = b.get(key).await.unwrap();
    *sealed.last_mut().unwrap() ^= 0x01;
    b.put_cas(key, sealed, etag.as_deref()).await.unwrap();
}

async fn is_stream_sealed(b: &Arc<dyn Backend>, key: &str) -> bool {
    let (sealed, etag) = b.get(key).await.unwrap();
    assert!(etag.is_some(), "{key} is missing");
    Cipher::is_stream_sealed(&sealed)
}

async fn exists(b: &Arc<dyn Backend>, key: &str) -> bool {
    b.get(key).await.unwrap().1.is_some()
}

#[tokio::test]
async fn lfs_objects_and_packs_survive_a_snapshot_restore_at_any_size() {
    let live_dir = tempfile::tempdir().unwrap();
    let backup_dir = tempfile::tempdir().unwrap();
    let restored_dir = tempfile::tempdir().unwrap();
    let live = backend(live_dir.path());
    let backup = backend(backup_dir.path());
    let restored = backend(restored_dir.path());
    let c = cipher();
    let backup_cipher = Arc::new(Cipher::derive("test passphrase", &[2u8; SALT_LEN]).unwrap());

    // Each class once under the 1 MiB in-memory cut-off and once over it.
    let objects = [
        ("repos/r1/lfs/small", patterned(100)),
        ("repos/r1/lfs/big", patterned(3 << 20)),
        ("repos/r1/objects/pack/pack-aa.pack", patterned(200)),
        ("repos/r1/objects/pack/pack-bb.pack", patterned((1 << 20) + 1)),
    ];
    for (key, body) in &objects {
        put_stream_sealed(&live, &c, key, body).await;
    }

    let mut run = fresh_run();
    let manifest = snapshot::capture(
        &live,
        &c,
        &backup,
        &backup_cipher,
        "snap1",
        &[Class::Git, Class::Lfs],
        2,
        &mut run,
    )
    .await
    .unwrap();
    assert!(manifest.complete, "capture failures: {:?}", run.failures);
    assert_eq!(manifest.objects, 4);

    let report = snapshot::restore(&backup, &backup_cipher, restored.as_ref(), &c, "snap1", false)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "restore failures: {:?}", report.failures);
    assert_eq!(report.restored, 4);

    for (key, body) in &objects {
        // The envelope is kept end to end: the live readers of these keys
        // only ever `open_stream` them.
        assert!(is_stream_sealed(&backup, &format!(".conf/backups/snap1/{key}")).await);
        assert!(is_stream_sealed(&restored, key).await);
        assert_eq!(&read_stream_sealed(&restored, &c, key).await, body, "{key}");
    }
}

#[tokio::test]
async fn a_large_single_shot_entity_keeps_its_envelope_through_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let restored_dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let restored = backend(restored_dir.path());
    let c = cipher();
    let key = ".conf/users/alice.json";
    let body = patterned(2 << 20);
    put_sealed(&src, &c, key, &body).await;

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();
    assert!(manifest.complete, "capture failures: {:?}", run.failures);
    let report = snapshot::restore(&src, &c, restored.as_ref(), &c, "snap1", false)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "restore failures: {:?}", report.failures);

    assert!(!is_stream_sealed(&restored, key).await);
    assert_eq!(read_sealed(&restored, &c, key).await.unwrap(), body);
}

#[tokio::test]
async fn a_corrupted_source_object_fails_its_capture() {
    let dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let c = cipher();
    let keys = ["repos/r1/lfs/small", "repos/r1/lfs/big"];
    put_stream_sealed(&src, &c, keys[0], &patterned(100)).await;
    put_stream_sealed(&src, &c, keys[1], &patterned(2 << 20)).await;
    for key in keys {
        corrupt(&src, key).await;
    }

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &[Class::Lfs], 2, &mut run)
        .await
        .unwrap();

    assert!(!manifest.complete);
    assert_eq!(manifest.objects, 0);
    assert_eq!(run.failures_total, 2);
    for key in keys {
        assert!(
            run.failures.iter().any(|f| f.starts_with(key)),
            "{key}: {:?}",
            run.failures
        );
        assert!(!exists(&src, &format!(".conf/backups/snap1/{key}")).await);
    }
}

#[tokio::test]
async fn a_corrupted_snapshot_copy_fails_its_restore_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let restored_dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let restored = backend(restored_dir.path());
    let c = cipher();
    let keys = [
        "repos/r1/objects/pack/pack-aa.pack",
        "repos/r1/objects/pack/pack-bb.pack",
    ];
    put_stream_sealed(&src, &c, keys[0], &patterned(100)).await;
    put_stream_sealed(&src, &c, keys[1], &patterned(2 << 20)).await;

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &[Class::Git], 2, &mut run)
        .await
        .unwrap();
    assert!(manifest.complete, "capture failures: {:?}", run.failures);
    for key in keys {
        corrupt(&src, &format!(".conf/backups/snap1/{key}")).await;
    }

    let report = snapshot::restore(&src, &c, restored.as_ref(), &c, "snap1", false)
        .await
        .unwrap();

    assert_eq!(report.restored, 0);
    assert_eq!(report.failures.len(), 2);
    for key in keys {
        assert!(
            report.failures.iter().any(|f| f.starts_with(key)),
            "{key}: {:?}",
            report.failures
        );
        assert!(!exists(&restored, key).await);
    }
}

#[tokio::test]
async fn a_dry_run_reports_corrupted_snapshot_copies_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let restored_dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let restored = backend(restored_dir.path());
    let c = cipher();
    let pack = "repos/r1/objects/pack/pack-aa.pack";
    let big_pack = "repos/r1/objects/pack/pack-bb.pack";
    let entity = ".conf/users/alice.json";
    put_stream_sealed(&src, &c, pack, &patterned(100)).await;
    put_stream_sealed(&src, &c, big_pack, &patterned(2 << 20)).await;
    put_sealed(&src, &c, entity, b"alice").await;

    let mut run = fresh_run();
    let classes = [Class::Git, Class::Entities];
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &classes, 2, &mut run)
        .await
        .unwrap();
    assert!(manifest.complete, "capture failures: {:?}", run.failures);
    for key in [pack, big_pack, entity] {
        corrupt(&src, &format!(".conf/backups/snap1/{key}")).await;
    }

    let report = snapshot::restore(&src, &c, restored.as_ref(), &c, "snap1", true)
        .await
        .unwrap();

    assert_eq!(report.restored, 0);
    assert_eq!(report.failures.len(), 3, "{:?}", report.failures);
    assert!(list_all_keys(restored.as_ref(), "").await.unwrap().is_empty());
}

#[tokio::test]
async fn restoring_into_a_bucket_without_a_data_key_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let empty = backend(dir.path());

    let err = snapshot::open_live_cipher(empty.as_ref(), "passphrase")
        .await
        .expect_err("an empty bucket has no data key to restore under");

    assert!(format!("{err:#}").contains("no data key"), "{err:#}");
    assert!(list_all_keys(empty.as_ref(), "").await.unwrap().is_empty());
}

#[tokio::test]
async fn a_pack_that_fails_its_restore_withholds_that_repos_refs() {
    let dir = tempfile::tempdir().unwrap();
    let restored_dir = tempfile::tempdir().unwrap();
    let src = backend(dir.path());
    let restored = backend(restored_dir.path());
    let c = cipher();
    put_stream_sealed(&src, &c, "repos/r1/objects/pack/pack-aa.pack", &patterned(200)).await;
    put_sealed(&src, &c, "repos/r1/refs.json", b"{\"new\":true}").await;
    put_stream_sealed(&src, &c, "repos/r2/objects/pack/pack-bb.pack", &patterned(200)).await;
    put_sealed(&src, &c, "repos/r2/refs.json", b"{\"r2\":true}").await;

    let mut run = fresh_run();
    let manifest = snapshot::capture(&src, &c, &src, &c, "snap1", &[Class::Git], 2, &mut run)
        .await
        .unwrap();
    assert!(manifest.complete, "capture failures: {:?}", run.failures);
    corrupt(&src, ".conf/backups/snap1/repos/r1/objects/pack/pack-aa.pack").await;
    put_sealed(&restored, &c, "repos/r1/refs.json", b"{\"old\":true}").await;

    for dry_run in [true, false] {
        let report = snapshot::restore(&src, &c, restored.as_ref(), &c, "snap1", dry_run)
            .await
            .unwrap();
        assert_eq!(report.failures.len(), 2, "{dry_run}: {:?}", report.failures);
        assert!(
            report.failures[1].starts_with("repos/r1/refs.json: withheld"),
            "{:?}",
            report.failures
        );
        assert_eq!(report.restored, 2, "{dry_run}: r2's pack and refs");
    }
    assert_eq!(
        read_sealed(&restored, &c, "repos/r1/refs.json").await.unwrap(),
        b"{\"old\":true}"
    );
    assert_eq!(
        read_sealed(&restored, &c, "repos/r2/refs.json").await.unwrap(),
        b"{\"r2\":true}"
    );
}
