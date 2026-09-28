use gitquarry_core::backup::inventory::Class;
use gitquarry_core::backup::report::{self, BackupRun, LIST_LIMIT, MAX_SAMPLES, Outcome, Trigger};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use s3db::LocalBackend;
use s3db_crypto::SALT_LEN;

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    });
    let cipher = Cipher::derive("test passphrase", &[9u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

#[test]
fn a_starting_run_is_recorded_as_running() {
    let run = BackupRun::starting("bucket:offsite", Trigger::Scheduled);
    assert_eq!(run.outcome, Outcome::Running);
    assert_eq!(run.finished_at, None);
    assert_eq!(run.dest, "bucket:offsite");
    assert!(run.started_at > 0);
    assert!(!run.id.is_empty());
    assert_eq!(run.duration_secs(), None);
}

#[test]
fn ids_sort_chronologically() {
    // Two runs a millisecond apart must order by id alone, since
    // `list_recent` sorts names without reading bodies.
    let mut ids: Vec<String> = Vec::new();
    for _ in 0..2 {
        ids.push(BackupRun::starting("self", Trigger::Cli).id);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(ids[0] < ids[1], "{ids:?} should be in ascending order");
}

#[test]
fn counters_split_copied_from_skipped_per_class() {
    let mut run = BackupRun::starting("self", Trigger::Cli);
    run.count(Class::Git, Some(1024));
    run.count(Class::Git, Some(512));
    run.count(Class::Git, None);
    run.count(Class::Lfs, Some(4096));

    let git = &run.classes["git"];
    assert_eq!((git.copied, git.skipped, git.bytes_copied), (2, 1, 1536));
    let lfs = &run.classes["lfs"];
    assert_eq!((lfs.copied, lfs.skipped, lfs.bytes_copied), (1, 0, 4096));
    // A class nothing touched gets no row at all.
    assert!(!run.classes.contains_key("audit"));

    let totals = run.totals();
    assert_eq!((totals.copied, totals.skipped, totals.bytes_copied), (3, 1, 5632));
}

#[test]
fn a_run_with_any_failure_finishes_partial() {
    let mut run = BackupRun::starting("self", Trigger::Manual);
    run.count(Class::Git, Some(10));
    run.count_failure(Class::Lfs, "repos/x/lfs/abc", "connection reset");
    run.finish();

    assert_eq!(run.outcome, Outcome::Partial);
    assert_eq!(run.failures_total, 1);
    assert_eq!(run.classes["lfs"].failed, 1);
    assert_eq!(run.failures, vec!["repos/x/lfs/abc: connection reset".to_string()]);
    assert!(run.finished_at.is_some());
}

#[test]
fn a_clean_run_finishes_ok() {
    let mut run = BackupRun::starting("self", Trigger::Manual);
    run.count(Class::Entities, Some(64));
    run.finish();
    assert_eq!(run.outcome, Outcome::Ok);
    assert!(run.duration_secs().is_some());
}

#[test]
fn a_run_that_never_started_records_why() {
    let mut run = BackupRun::starting("bucket:typo", Trigger::Scheduled);
    run.fail("bucket 'typo' not configured".to_string());
    assert_eq!(run.outcome, Outcome::Failed);
    assert_eq!(run.error.as_deref(), Some("bucket 'typo' not configured"));
}

#[test]
fn failure_and_unknown_samples_are_capped_but_totals_are_not() {
    let mut run = BackupRun::starting("self", Trigger::Cli);
    for i in 0..(MAX_SAMPLES * 3) {
        run.count_failure(Class::Git, &format!("repos/x/objects/pack/pack-{i}.pack"), "boom");
        run.note_unknown(&format!("brand-new/{i}.json"));
    }
    assert_eq!(run.failures.len(), MAX_SAMPLES);
    assert_eq!(run.unknown_keys.len(), MAX_SAMPLES);
    assert_eq!(run.failures_total, (MAX_SAMPLES * 3) as u64);
    assert_eq!(run.classes["git"].failed, (MAX_SAMPLES * 3) as u64);
}

#[tokio::test]
async fn saved_runs_come_back_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: &dyn Backend = &backend;

    let mut ids = Vec::new();
    for _ in 0..3 {
        let mut run = BackupRun::starting("self", Trigger::Cli);
        run.finish();
        ids.push(run.id.clone());
        report::save(backend, &cipher, &run).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let listed = report::list_recent(backend, &cipher, LIST_LIMIT).await.unwrap();
    let listed_ids: Vec<String> = listed.iter().map(|r| r.id.clone()).collect();
    ids.reverse();
    assert_eq!(listed_ids, ids);
}

#[tokio::test]
async fn saving_the_same_run_twice_overwrites_it() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: &dyn Backend = &backend;

    // The real lifecycle: `Running` at the start, final outcome at the end,
    // same id both times.
    let mut run = BackupRun::starting("self", Trigger::Scheduled);
    report::save(backend, &cipher, &run).await.unwrap();
    run.count(Class::Git, Some(1));
    run.finish();
    report::save(backend, &cipher, &run).await.unwrap();

    let listed = report::list_recent(backend, &cipher, LIST_LIMIT).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].outcome, Outcome::Ok);
}

#[tokio::test]
async fn list_recent_honors_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: &dyn Backend = &backend;

    for _ in 0..4 {
        let run = BackupRun::starting("self", Trigger::Cli);
        report::save(backend, &cipher, &run).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(report::list_recent(backend, &cipher, 2).await.unwrap().len(), 2);
}

#[tokio::test]
async fn pruning_keeps_the_newest_records_only() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: &dyn Backend = &backend;

    let mut ids = Vec::new();
    for _ in 0..(LIST_LIMIT + 3) {
        let run = BackupRun::starting("self", Trigger::Cli);
        ids.push(run.id.clone());
        report::save(backend, &cipher, &run).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    assert_eq!(report::prune_old_runs(backend, &cipher).await.unwrap(), 3);
    let left = report::list_recent(backend, &cipher, LIST_LIMIT * 2).await.unwrap();
    assert_eq!(left.len(), LIST_LIMIT);
    // The three oldest went, not three arbitrary ones.
    let kept: Vec<String> = left.iter().map(|r| r.id.clone()).collect();
    for dropped in &ids[..3] {
        assert!(!kept.contains(dropped), "{dropped} should have been pruned");
    }
}

#[tokio::test]
async fn pruning_a_short_history_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: &dyn Backend = &backend;

    let run = BackupRun::starting("self", Trigger::Cli);
    report::save(backend, &cipher, &run).await.unwrap();
    assert_eq!(report::prune_old_runs(backend, &cipher).await.unwrap(), 0);
    assert_eq!(
        report::list_recent(backend, &cipher, LIST_LIMIT).await.unwrap().len(),
        1
    );
}
