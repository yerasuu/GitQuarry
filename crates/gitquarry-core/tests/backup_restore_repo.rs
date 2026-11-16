//! `backup::restore_repo`: one purged repo back from a real backup run.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::backup::dest::Source;
use gitquarry_core::backup::report::{Outcome, Trigger};
use gitquarry_core::backup::restore_repo::{EntitySource, restore_repo};
use gitquarry_core::backup::schedule;
use gitquarry_core::domain::repo;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{self, Backend, list_all_keys};
use gitquarry_test_support::{Work, backend, git};
use std::sync::Arc;

async fn read(b: &Arc<dyn Backend>, key: &str) -> Option<Vec<u8>> {
    let (bytes, etag) = b.get(key).await.unwrap();
    etag.map(|_| bytes)
}

#[tokio::test]
async fn a_purged_repo_comes_back_from_the_replica_and_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let backup_dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let live = ctx.backend();
    // The test cipher is derived directly, so the bucket has no real
    // encryption config; a replica only needs one to exist and match.
    live.put_cas(".conf/encryption.json", b"{\"salt\":\"test\"}".to_vec(), None)
        .await
        .unwrap();

    let resolved = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    let head = work.commit(&[("README.md", "hello\n")], "first").await;
    work.push(&resolved, "refs/heads/main").await;
    let uid = resolved.uid.clone();
    let prefix = format!("repos/{uid}/");

    // Soft-deleted before the backup, as a repo is during its grace window,
    // so the backed-up config carries `deleted_at`.
    repo::delete(&ctx, &resolved, "team/proj").await.unwrap();
    let source = Source {
        bucket_name: &ctx.bucket,
        backend: &live,
        cipher: &ctx.cipher,
        config_path: &ctx.config_path,
    };
    let dest = format!("path:{}", backup_dir.path().display());
    let run = schedule::run_for(&source, Trigger::Cli, Some(&dest), None)
        .await
        .unwrap();
    assert_eq!(run.outcome, Outcome::Ok, "backup failed: {:?}", run.error);
    let snapshot_id = run.snapshot_id.clone().expect("the run took a snapshot");

    repo::purge(&ctx, &resolved, "team/proj").await.unwrap();
    assert!(list_all_keys(live.as_ref(), &prefix).await.unwrap().is_empty());

    let backup = backend(backup_dir.path());
    let dry = restore_repo(&backup, &live, &ctx.cipher, &uid, None, true)
        .await
        .unwrap();
    assert_eq!(dry.entities_from, EntitySource::Snapshot(snapshot_id));
    assert!(dry.replica_objects >= 2, "pack and index: {dry:?}");
    assert!(dry.entity_objects >= 1, "config.json: {dry:?}");
    assert!(dry.refs_restored && dry.undeleted && dry.failures.is_empty(), "{dry:?}");
    assert!(
        list_all_keys(live.as_ref(), &prefix).await.unwrap().is_empty(),
        "a dry run must not write"
    );

    let applied = restore_repo(&backup, &live, &ctx.cipher, &uid, None, false)
        .await
        .unwrap();
    assert!(applied.failures.is_empty(), "{applied:?}");
    assert_eq!(applied.replica_objects, dry.replica_objects);
    assert_eq!(applied.entity_objects, dry.entity_objects);
    assert!(applied.refs_restored);

    // Git data goes back as the very same ciphertext.
    for key in list_all_keys(backup.as_ref(), &prefix).await.unwrap() {
        if key.ends_with("/refs.json") || key.contains("/objects/pack/") {
            assert_eq!(read(&live, &key).await, read(&backup, &key).await, "{key}");
        }
    }
    let (config, etag) = storage::settings::get_repo_config(live.as_ref(), uid.as_str(), &ctx.cipher)
        .await
        .unwrap();
    assert!(etag.is_some());
    assert_eq!(config.full_name, "team/proj");
    assert!(config.deleted_at.is_none(), "a restored repo must serve again");

    let restored = ResolvedRepo {
        remote: ctx.remote_for_uid(&uid),
        uid,
    };
    let mirror = ctx.open_mirror(&restored).await.unwrap();
    assert_eq!(git(&mirror.path, &["rev-parse", "refs/heads/main"]).await, head);
}

#[tokio::test]
async fn a_backup_of_another_bucket_is_refused_before_writing() {
    let live_dir = tempfile::tempdir().unwrap();
    let backup_dir = tempfile::tempdir().unwrap();
    let live = backend(live_dir.path());
    let backup = backend(backup_dir.path());
    live.put_cas(".conf/encryption.json", b"{\"salt\":\"live\"}".to_vec(), None)
        .await
        .unwrap();
    backup
        .put_cas(".conf/encryption.json", b"{\"salt\":\"other\"}".to_vec(), None)
        .await
        .unwrap();
    let uid = gitquarry_core::validate::validate_repo_uid("0123456789abcdef0123456789abcdef").unwrap();

    let err = restore_repo(&backup, &live, &gitquarry_test_support::cipher(), &uid, None, false)
        .await
        .expect_err("a replica of another bucket can't be opened here");
    assert!(format!("{err:#}").contains("same .conf/encryption.json"), "{err:#}");
    assert!(list_all_keys(live.as_ref(), "repos/").await.unwrap().is_empty());
}

/// Printable but incompressible enough (xorshift, 64 symbols) that git's
/// zlib can't shrink a few MiB of it under the 1 MiB inline cut-off.
fn noise(len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            char::from(ALPHABET[(state % 64) as usize])
        })
        .collect()
}

/// A repo backed up and then purged; the tempdirs live as long as it does.
struct Purged {
    _live_dir: tempfile::TempDir,
    _backup_dir: tempfile::TempDir,
    ctx: AppContext,
    live: Arc<dyn Backend>,
    backup: Arc<dyn Backend>,
    uid: Uid,
}

/// One repo pushed with `content` in a file, backed up, then purged.
async fn purged_repo_with(content: &str) -> Purged {
    let dir = tempfile::tempdir().unwrap();
    let backup_dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let live = ctx.backend();
    live.put_cas(".conf/encryption.json", b"{\"salt\":\"test\"}".to_vec(), None)
        .await
        .unwrap();
    let resolved = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    work.commit(&[("data.txt", content)], "first").await;
    work.push(&resolved, "refs/heads/main").await;
    repo::delete(&ctx, &resolved, "team/proj").await.unwrap();
    let source = Source {
        bucket_name: &ctx.bucket,
        backend: &live,
        cipher: &ctx.cipher,
        config_path: &ctx.config_path,
    };
    let dest = format!("path:{}", backup_dir.path().display());
    let run = schedule::run_for(&source, Trigger::Cli, Some(&dest), None)
        .await
        .unwrap();
    assert_eq!(run.outcome, Outcome::Ok, "backup failed: {:?}", run.error);
    repo::purge(&ctx, &resolved, "team/proj").await.unwrap();
    Purged {
        backup: backend(backup_dir.path()),
        _live_dir: dir,
        _backup_dir: backup_dir,
        ctx,
        live,
        uid: resolved.uid,
    }
}

async fn a_corrupt_replica_pack_is_refused(content: &str, min_pack_bytes: usize) {
    let Purged {
        ctx, live, backup, uid, ..
    } = &purged_repo_with(content).await;
    let prefix = format!("repos/{uid}/");
    let pack = list_all_keys(backup.as_ref(), &prefix)
        .await
        .unwrap()
        .into_iter()
        .find(|key| key.ends_with(".pack"))
        .expect("the replica holds the pack");
    let (mut sealed, etag) = backup.get(&pack).await.unwrap();
    assert!(sealed.len() >= min_pack_bytes, "pack is {} bytes", sealed.len());
    let last = sealed.len() - 1;
    sealed[last] ^= 0x01;
    backup.put_cas(&pack, sealed, etag.as_deref()).await.unwrap();

    let report = restore_repo(backup, live, &ctx.cipher, uid, None, false).await.unwrap();
    assert!(!report.refs_restored, "{report:?}");
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert!(report.failures[0].starts_with(&format!("{pack}: ")), "{report:?}");
    assert!(report.failures[0].contains("doesn't decrypt"), "{report:?}");
    assert!(read(live, &format!("{prefix}refs.json")).await.is_none());
    assert!(
        read(live, &pack).await.is_none(),
        "a pack that doesn't open is never written"
    );
}

#[tokio::test]
async fn a_small_corrupt_replica_pack_fails_the_restore_without_refs() {
    a_corrupt_replica_pack_is_refused("hello\n", 0).await;
}

#[tokio::test]
async fn a_large_corrupt_replica_pack_fails_the_restore_without_refs() {
    a_corrupt_replica_pack_is_refused(&noise(3 << 20), (1 << 20) + 1).await;
}

#[tokio::test]
async fn a_large_clean_replica_pack_restores() {
    let Purged {
        ctx, live, backup, uid, ..
    } = &purged_repo_with(&noise(3 << 20)).await;
    let report = restore_repo(backup, live, &ctx.cipher, uid, None, false).await.unwrap();
    assert!(report.failures.is_empty() && report.refs_restored, "{report:?}");
    assert!(report.replica_bytes > 1 << 20, "{report:?}");
    let prefix = format!("repos/{uid}/");
    for key in list_all_keys(backup.as_ref(), &prefix).await.unwrap() {
        if key.ends_with("/refs.json") || key.contains("/objects/pack/") {
            assert_eq!(read(live, &key).await, read(backup, &key).await, "{key}");
        }
    }
}
