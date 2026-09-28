use gitquarry_core::backup::inventory::Selection;
use gitquarry_core::backup::replica;
use gitquarry_core::backup::report::{BackupRun, Trigger};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::storage::Backend;
use s3db::LocalBackend;
use std::sync::Arc;

const ENCRYPTION_KEY: &str = ".conf/encryption.json";

fn backend(root: &std::path::Path) -> Arc<dyn Backend> {
    Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }))
}

/// The replica path never decrypts anything, so these tests write raw bytes
/// straight through the backend — a `Cipher` would only obscure what's being
/// asserted.
async fn put(b: &Arc<dyn Backend>, key: &str, body: &[u8]) {
    let (_, etag) = b.get(key).await.unwrap();
    b.put_cas(key, body.to_vec(), etag.as_deref()).await.unwrap();
}

async fn read(b: &Arc<dyn Backend>, key: &str) -> Option<Vec<u8>> {
    let (bytes, etag) = b.get(key).await.unwrap();
    etag.map(|_| bytes)
}

async fn seeded_source(root: &std::path::Path) -> Arc<dyn Backend> {
    let src = backend(root);
    put(&src, ENCRYPTION_KEY, b"{\"salt\":\"abc\"}").await;
    src
}

fn fresh_run() -> BackupRun {
    BackupRun::starting("path:/tmp/replica", Trigger::Cli)
}

#[tokio::test]
async fn an_empty_destination_gets_the_source_encryption_config() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());

    replica::ensure_same_data_key(src.as_ref(), dest.as_ref())
        .await
        .unwrap();
    assert_eq!(read(&dest, ENCRYPTION_KEY).await.unwrap(), b"{\"salt\":\"abc\"}");
}

#[tokio::test]
async fn a_destination_with_a_different_data_key_is_refused() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&dest, ENCRYPTION_KEY, b"{\"salt\":\"different\"}").await;

    let err = replica::ensure_same_data_key(src.as_ref(), dest.as_ref())
        .await
        .expect_err("a different encryption config must not be replicated into");
    assert!(
        format!("{err:#}").contains("different encryption config"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn a_non_empty_destination_without_an_encryption_config_is_refused() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&dest, "repos/other/refs.json", b"someone else's bucket").await;

    let err = replica::ensure_same_data_key(src.as_ref(), dest.as_ref())
        .await
        .expect_err("an unrelated non-empty bucket must not be claimed as a replica");
    assert!(
        format!("{err:#}").contains("isn't a gitquarry"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn an_uninitialized_source_is_refused() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = backend(src_dir.path());
    let dest = backend(dest_dir.path());

    let err = replica::ensure_same_data_key(src.as_ref(), dest.as_ref())
        .await
        .expect_err("a source with no encryption config has nothing to replicate");
    assert!(
        format!("{err:#}").contains("never initialized"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn every_selected_object_lands_at_the_same_key() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());

    put(&src, "repos/r1/refs.json", b"refs").await;
    put(&src, "repos/r1/objects/pack/pack-aa.pack", b"packdata").await;
    put(&src, "repos/r1/lfs/oid1", b"lfsdata").await;
    put(&src, "repos/r1/.conf/config.json", b"{}").await;
    put(&src, ".conf/users/alice.json", b"alice").await;
    put(&src, "avatars/alice", b"png").await;

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 4, &mut run)
        .await
        .unwrap();
    run.finish();

    for key in [
        "repos/r1/refs.json",
        "repos/r1/objects/pack/pack-aa.pack",
        "repos/r1/lfs/oid1",
        "repos/r1/.conf/config.json",
        ".conf/users/alice.json",
        "avatars/alice",
    ] {
        assert!(read(&dest, key).await.is_some(), "{key} should have been replicated");
    }
    assert_eq!(run.failures_total, 0);
    assert!(run.totals().copied >= 6, "copied {} objects", run.totals().copied);
    // The only thing skipped is the encryption config: `ensure_same_data_key`
    // already installed it before the walk started, so the walk finds it
    // byte-identical.
    assert_eq!(run.classes["bootstrap"].skipped, 1);
    for class in ["git", "lfs", "entities", "avatars"] {
        assert_eq!(run.classes[class].skipped, 0, "{class} should have copied everything");
    }
}

#[tokio::test]
async fn derived_data_is_left_out_by_default_and_included_on_request() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&src, "repos/r1/search-index/term.json", b"idx").await;

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();
    assert!(read(&dest, "repos/r1/search-index/term.json").await.is_none());

    let with_derived = Selection {
        derived: true,
        ..Selection::default()
    };
    let mut run = fresh_run();
    replica::sync(&src, &dest, with_derived, 2, &mut run).await.unwrap();
    assert!(read(&dest, "repos/r1/search-index/term.json").await.is_some());
}

#[tokio::test]
async fn the_backup_systems_own_output_is_never_copied() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&src, ".conf/backup-runs/00001-aaaa.json", b"report").await;
    put(&src, ".conf/backups/old/repos/r1/refs.json", b"snapshot").await;
    put(&src, ".conf/txn/t1.json", b"txn").await;

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();

    assert!(read(&dest, ".conf/backup-runs/00001-aaaa.json").await.is_none());
    assert!(read(&dest, ".conf/backups/old/repos/r1/refs.json").await.is_none());
    assert!(read(&dest, ".conf/txn/t1.json").await.is_none());
}

#[tokio::test]
async fn an_unchanged_immutable_object_is_skipped_on_the_second_run() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&src, "repos/r1/objects/pack/pack-aa.pack", b"packdata").await;

    let mut first = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut first)
        .await
        .unwrap();
    assert_eq!(first.classes["git"].copied, 1);
    assert_eq!(first.classes["git"].skipped, 0);

    let mut second = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut second)
        .await
        .unwrap();
    assert_eq!(second.classes["git"].copied, 0);
    assert_eq!(second.classes["git"].skipped, 1);
}

#[tokio::test]
async fn a_mutable_object_rewritten_to_the_same_length_is_still_replicated() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    // Same length, different content -- the case a size-only incremental
    // check would silently miss, leaving the replica stale.
    put(&src, "repos/r1/refs.json", b"aaaa").await;

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();
    assert_eq!(read(&dest, "repos/r1/refs.json").await.unwrap(), b"aaaa");

    put(&src, "repos/r1/refs.json", b"bbbb").await;
    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();
    assert_eq!(read(&dest, "repos/r1/refs.json").await.unwrap(), b"bbbb");
    assert_eq!(run.classes["git"].copied, 1);
}

#[tokio::test]
async fn an_unchanged_mutable_object_is_read_but_not_rewritten() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&src, ".conf/users/alice.json", b"alice").await;

    let mut first = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut first)
        .await
        .unwrap();
    assert_eq!(first.classes["entities"].copied, 1);

    // Byte-identical: counted as skipped even though it's a mutable class,
    // because the small-object path compares content rather than size.
    let mut second = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut second)
        .await
        .unwrap();
    assert_eq!(second.classes["entities"].copied, 0);
    assert_eq!(second.classes["entities"].skipped, 1);
}

#[tokio::test]
async fn destination_only_keys_are_never_deleted() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&dest, ENCRYPTION_KEY, b"{\"salt\":\"abc\"}").await;
    // A pack a `repack` has since replaced at the source, and a repo that
    // was purged: both must survive at the replica.
    put(&dest, "repos/r1/objects/pack/pack-old.pack", b"superseded").await;
    put(&dest, "repos/gone/refs.json", b"purged repo").await;
    put(&src, "repos/r1/objects/pack/pack-new.pack", b"current").await;

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();

    assert!(read(&dest, "repos/r1/objects/pack/pack-old.pack").await.is_some());
    assert!(read(&dest, "repos/gone/refs.json").await.is_some());
    assert!(read(&dest, "repos/r1/objects/pack/pack-new.pack").await.is_some());
}

#[tokio::test]
async fn unclassified_keys_are_copied_and_reported() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    put(&src, "brand-new-feature/thing.json", b"data").await;

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();

    assert!(read(&dest, "brand-new-feature/thing.json").await.is_some());
    assert_eq!(run.unknown_keys, vec!["brand-new-feature/thing.json".to_string()]);
    assert_eq!(run.classes["unknown"].copied, 1);
}

#[tokio::test]
async fn a_large_object_streams_through_and_is_size_checked() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());
    // Over the inline threshold, so it takes the streaming path.
    let big = vec![7u8; (1 << 20) + 1];
    put(&src, "repos/r1/lfs/bigoid", &big).await;

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut run)
        .await
        .unwrap();
    assert_eq!(read(&dest, "repos/r1/lfs/bigoid").await.unwrap().len(), big.len());
    assert_eq!(run.classes["lfs"].copied, 1);

    // Second run: present, same size, immutable -> skipped without a read.
    let mut second = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 2, &mut second)
        .await
        .unwrap();
    assert_eq!(second.classes["lfs"].skipped, 1);
    assert_eq!(second.classes["lfs"].copied, 0);
}

#[tokio::test]
async fn a_run_spanning_more_than_one_listing_page_copies_everything() {
    let src_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let src = seeded_source(src_dir.path()).await;
    let dest = backend(dest_dir.path());

    // Enough objects that the merge walks several batches, with keys whose
    // lexicographic order is deliberately not their insertion order.
    for i in 0..40 {
        put(&src, &format!("repos/r1/lfs/oid{:03}", (i * 7) % 40), b"x").await;
    }

    let mut run = fresh_run();
    replica::sync(&src, &dest, Selection::default(), 3, &mut run)
        .await
        .unwrap();
    assert_eq!(run.classes["lfs"].copied, 40);
    for i in 0..40 {
        let key = format!("repos/r1/lfs/oid{i:03}");
        assert!(read(&dest, &key).await.is_some(), "{key} missing");
    }
}
