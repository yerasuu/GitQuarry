use gitquarry_core::backup::inventory::Selection;
use gitquarry_core::backup::prune;
use gitquarry_core::backup::replica;
use gitquarry_core::backup::report::{BackupRun, Trigger};
use gitquarry_core::backup::snapshot::{self, DEFAULT_CLASSES};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, CompressionStrategy, SALT_LEN};
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::local::LocalBackend;
use std::sync::Arc;

const ENCRYPTION_KEY: &str = ".conf/encryption.json";
const DAY: u64 = 86400;

fn backend(root: &std::path::Path) -> Arc<dyn Backend> {
    Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }))
}

fn cipher() -> Arc<Cipher> {
    Arc::new(Cipher::derive("test passphrase", &[11u8; SALT_LEN]).unwrap())
}

async fn put(b: &Arc<dyn Backend>, key: &str, body: &[u8]) {
    let (_, etag) = b.get(key).await.unwrap();
    b.put_cas(key, body.to_vec(), etag.as_deref()).await.unwrap();
}

async fn put_sealed(b: &Arc<dyn Backend>, c: &Cipher, key: &str, body: &[u8]) {
    let sealed = c.seal(key, body, CompressionStrategy::None).unwrap();
    let (_, etag) = b.get(key).await.unwrap();
    b.put_cas(key, sealed, etag.as_deref()).await.unwrap();
}

async fn exists(b: &Arc<dyn Backend>, key: &str) -> bool {
    b.get(key).await.unwrap().1.is_some()
}

async fn delete(b: &Arc<dyn Backend>, key: &str) {
    b.delete(key).await.unwrap();
}

/// A source and a destination already in sync, plus one key at the
/// destination the source has since lost.
async fn synced_pair(
    src_root: &std::path::Path,
    dest_root: &std::path::Path,
) -> (Arc<dyn Backend>, Arc<dyn Backend>, Arc<Cipher>) {
    let src = backend(src_root);
    let dest = backend(dest_root);
    let c = cipher();
    put(&src, ENCRYPTION_KEY, b"{\"salt\":\"abc\"}").await;
    put(&src, "repos/r1/objects/pack/pack-keep.pack", b"kept").await;
    put(&src, "repos/r1/objects/pack/pack-gone.pack", b"doomed").await;

    let mut run = BackupRun::starting("path:dest", Trigger::Cli);
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();
    // Now it's only at the destination -- a repack replaced it, say.
    delete(&src, "repos/r1/objects/pack/pack-gone.pack").await;
    (src, dest, c)
}

#[tokio::test]
async fn the_first_pass_only_notes_an_orphan() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let (src, dest, c) = synced_pair(src_dir.path(), dest_dir.path()).await;

    let outcome = prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000).await.unwrap();
    assert_eq!(outcome.noted, 1);
    assert_eq!(outcome.deleted, 0);
    // Still there, which is the entire point of the grace period.
    assert!(exists(&dest, "repos/r1/objects/pack/pack-gone.pack").await);
    assert!(exists(&dest, "repos/r1/objects/pack/pack-keep.pack").await);
}

#[tokio::test]
async fn a_second_pass_inside_the_grace_period_still_deletes_nothing() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let (src, dest, c) = synced_pair(src_dir.path(), dest_dir.path()).await;

    prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000).await.unwrap();
    let outcome = prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000 + DAY)
        .await
        .unwrap();
    assert_eq!(outcome.deleted, 0);
    assert_eq!(outcome.noted, 0, "the note already exists, it shouldn't be re-made");
    assert!(exists(&dest, "repos/r1/objects/pack/pack-gone.pack").await);
}

#[tokio::test]
async fn a_pass_past_the_grace_period_deletes_the_orphan_only() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let (src, dest, c) = synced_pair(src_dir.path(), dest_dir.path()).await;

    prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000).await.unwrap();
    let outcome = prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000 + 8 * DAY)
        .await
        .unwrap();

    assert_eq!(outcome.deleted, 1);
    assert!(!exists(&dest, "repos/r1/objects/pack/pack-gone.pack").await);
    // Everything the source still has is untouched, encryption config included.
    assert!(exists(&dest, "repos/r1/objects/pack/pack-keep.pack").await);
    assert!(exists(&dest, ENCRYPTION_KEY).await);
}

#[tokio::test]
async fn an_orphan_that_comes_back_loses_its_note_and_survives() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let (src, dest, c) = synced_pair(src_dir.path(), dest_dir.path()).await;

    prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000).await.unwrap();
    // Restored at the source (the repo was un-purged, the pack re-pushed).
    put(&src, "repos/r1/objects/pack/pack-gone.pack", b"doomed").await;

    let outcome = prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000 + DAY)
        .await
        .unwrap();
    assert_eq!(outcome.cleared, 1);
    assert_eq!(outcome.deleted, 0);

    // And because the note is gone, a later pass has to start the clock over
    // rather than delete on the strength of the old note.
    delete(&src, "repos/r1/objects/pack/pack-gone.pack").await;
    let outcome = prune::prune_replica(&src, &dest, &c, 7 * DAY, 1_000_000 + 20 * DAY)
        .await
        .unwrap();
    assert_eq!(outcome.deleted, 0, "the grace period must measure continuous absence");
    assert_eq!(outcome.noted, 1);
    assert!(exists(&dest, "repos/r1/objects/pack/pack-gone.pack").await);
}

#[tokio::test]
async fn a_zero_grace_still_takes_two_passes() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let (src, dest, c) = synced_pair(src_dir.path(), dest_dir.path()).await;

    let first = prune::prune_replica(&src, &dest, &c, 0, 1_000_000).await.unwrap();
    assert_eq!(first.deleted, 0, "nothing is deleted the first time it's seen");
    let second = prune::prune_replica(&src, &dest, &c, 0, 1_000_000).await.unwrap();
    assert_eq!(second.deleted, 1);
}

#[tokio::test]
async fn the_ledger_and_previous_snapshots_are_never_treated_as_orphans() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = backend(src_dir.path());
    let dest = backend(dest_dir.path());
    let c = cipher();
    put(&src, ENCRYPTION_KEY, b"{\"salt\":\"abc\"}").await;
    put(&dest, ENCRYPTION_KEY, b"{\"salt\":\"abc\"}").await;
    // Destination-only by design: a snapshot and a run record.
    put_sealed(&dest, &c, ".conf/backups/snap1/.conf/users/alice.json", b"alice").await;
    put_sealed(&dest, &c, ".conf/backup-runs/00001-aaaa.json", b"report").await;

    let outcome = prune::prune_replica(&src, &dest, &c, 0, 1_000_000).await.unwrap();
    assert_eq!(outcome.noted, 0);
    let outcome = prune::prune_replica(&src, &dest, &c, 0, 1_000_000).await.unwrap();
    assert_eq!(outcome.deleted, 0);
    assert!(exists(&dest, ".conf/backups/snap1/.conf/users/alice.json").await);
    assert!(exists(&dest, ".conf/backup-runs/00001-aaaa.json").await);
}

#[tokio::test]
async fn snapshot_retention_keeps_the_newest_and_deletes_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let dest = backend(dir.path());
    let c = cipher();
    put_sealed(&dest, &c, ".conf/users/alice.json", b"alice").await;
    for id in ["snap1", "snap2", "snap3", "snap4"] {
        let mut run = BackupRun::starting("self", Trigger::Cli);
        snapshot::capture(&dest, &c, &dest, &c, id, &DEFAULT_CLASSES, 2, &mut run)
            .await
            .unwrap();
    }

    assert_eq!(prune::prune_snapshots(&dest, &c, 2).await.unwrap(), 2);
    let left: Vec<String> = snapshot::list(&dest, &c)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(left, vec!["snap4", "snap3"]);
    // And the deleted snapshots' objects went with them.
    assert!(!exists(&dest, ".conf/backups/snap1/.conf/users/alice.json").await);
}

#[tokio::test]
async fn snapshot_retention_never_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let dest = backend(dir.path());
    let c = cipher();
    put_sealed(&dest, &c, ".conf/users/alice.json", b"alice").await;
    for id in ["snap1", "snap2"] {
        let mut run = BackupRun::starting("self", Trigger::Cli);
        snapshot::capture(&dest, &c, &dest, &c, id, &DEFAULT_CLASSES, 2, &mut run)
            .await
            .unwrap();
    }

    // A configured `0` is treated as 1: a retention setting shouldn't be
    // able to wipe every snapshot in one pass.
    assert_eq!(prune::prune_snapshots(&dest, &c, 0).await.unwrap(), 1);
    assert_eq!(snapshot::list(&dest, &c).await.unwrap().len(), 1);
}

#[tokio::test]
async fn snapshot_retention_does_nothing_when_under_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let dest = backend(dir.path());
    let c = cipher();
    put_sealed(&dest, &c, ".conf/users/alice.json", b"alice").await;
    let mut run = BackupRun::starting("self", Trigger::Cli);
    snapshot::capture(&dest, &c, &dest, &c, "snap1", &DEFAULT_CLASSES, 2, &mut run)
        .await
        .unwrap();

    assert_eq!(prune::prune_snapshots(&dest, &c, 5).await.unwrap(), 0);
    assert_eq!(snapshot::list(&dest, &c).await.unwrap().len(), 1);
}

#[tokio::test]
async fn audit_retention_drops_whole_old_months() {
    let dir = tempfile::tempdir().unwrap();
    let dest = backend(dir.path());
    for month in ["2026-05", "2026-06", "2026-07", "2026-08"] {
        put(&dest, &format!(".conf/audit/{month}/00001-aaaa.json"), b"entry").await;
        put(&dest, &format!(".conf/audit/{month}/00002-bbbb.json"), b"entry").await;
    }

    assert_eq!(prune::prune_audit_months(&dest, 2).await.unwrap(), 2);
    for kept in ["2026-07", "2026-08"] {
        assert!(exists(&dest, &format!(".conf/audit/{kept}/00001-aaaa.json")).await);
    }
    for dropped in ["2026-05", "2026-06"] {
        assert!(!exists(&dest, &format!(".conf/audit/{dropped}/00001-aaaa.json")).await);
        assert!(!exists(&dest, &format!(".conf/audit/{dropped}/00002-bbbb.json")).await);
    }
}

#[tokio::test]
async fn audit_retention_does_nothing_when_under_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let dest = backend(dir.path());
    put(&dest, ".conf/audit/2026-08/00001-aaaa.json", b"entry").await;
    assert_eq!(prune::prune_audit_months(&dest, 12).await.unwrap(), 0);
    assert!(exists(&dest, ".conf/audit/2026-08/00001-aaaa.json").await);
}

#[tokio::test]
async fn audit_retention_leaves_pre_month_bucket_entries_alone() {
    let dir = tempfile::tempdir().unwrap();
    let dest = backend(dir.path());
    // Written before month bucketing existed: flat under the root, no month
    // segment to judge it by.
    put(&dest, ".conf/audit/00001-legacy.json", b"entry").await;
    for month in ["2026-07", "2026-08"] {
        put(&dest, &format!(".conf/audit/{month}/00001-aaaa.json"), b"entry").await;
    }

    assert_eq!(prune::prune_audit_months(&dest, 1).await.unwrap(), 1);
    assert!(exists(&dest, ".conf/audit/00001-legacy.json").await);
    assert!(exists(&dest, ".conf/audit/2026-08/00001-aaaa.json").await);
}
