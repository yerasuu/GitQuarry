//! `backup::restore_repo`: one purged repo back from a real backup run.

use gitquarry_core::app::ResolvedRepo;
use gitquarry_core::backup::dest::Source;
use gitquarry_core::backup::report::{Outcome, Trigger};
use gitquarry_core::backup::restore_repo::{EntitySource, restore_repo};
use gitquarry_core::backup::schedule;
use gitquarry_core::domain::repo;
use gitquarry_core::storage::entities::Visibility;
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
