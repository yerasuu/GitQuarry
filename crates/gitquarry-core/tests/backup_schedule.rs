use gitquarry_core::app::AppContext;
use gitquarry_core::backup::report::{self, Outcome, Trigger};
use gitquarry_core::backup::schedule::{self, DEFAULT_INTERVAL, MIN_INTERVAL};
use gitquarry_core::storage::entities::ServerSettings;
use gitquarry_core::storage::settings::put_settings;
use gitquarry_test_support::ctx;
use std::time::Duration;

async fn save_settings(ctx: &AppContext, settings: &ServerSettings) {
    let backend = ctx.backend();
    let (_, etag) = gitquarry_core::storage::settings::get_settings(backend.as_ref(), &ctx.cipher)
        .await
        .unwrap();
    put_settings(backend.as_ref(), settings, etag.as_deref(), &ctx.cipher)
        .await
        .unwrap();
}

#[test]
fn an_unset_interval_uses_the_default() {
    assert_eq!(schedule::interval(&ServerSettings::default()), DEFAULT_INTERVAL);
}

#[test]
fn a_configured_interval_is_honored() {
    let settings = ServerSettings {
        backup_interval: Some("6h".to_string()),
        ..Default::default()
    };
    assert_eq!(schedule::interval(&settings), Duration::from_secs(6 * 3600));
}

#[test]
fn an_interval_below_the_floor_is_clamped_up() {
    let settings = ServerSettings {
        backup_interval: Some("1s".to_string()),
        ..Default::default()
    };
    // A typo mustn't become a permanent listing loop against the provider.
    assert_eq!(schedule::interval(&settings), MIN_INTERVAL);
}

#[test]
fn an_unparseable_interval_falls_back_to_the_default_rather_than_off() {
    let settings = ServerSettings {
        backup_interval: Some("whenever".to_string()),
        ..Default::default()
    };
    assert_eq!(schedule::interval(&settings), DEFAULT_INTERVAL);
}

#[tokio::test]
async fn a_run_with_no_destination_configured_is_recorded_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());

    let run = schedule::run_once(&ctx, Trigger::Cli).await.unwrap();
    assert_eq!(run.outcome, Outcome::Failed);
    assert!(
        run.error
            .as_deref()
            .unwrap_or_default()
            .contains("no backup destination"),
        "unexpected error: {:?}",
        run.error
    );

    // And it's readable afterwards, which is the whole reason a failure is
    // recorded rather than only logged.
    let stored = report::list_recent(ctx.backend().as_ref(), &ctx.cipher, 5)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].outcome, Outcome::Failed);
}

#[tokio::test]
async fn a_run_against_an_unparseable_destination_is_recorded_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    save_settings(
        &ctx,
        &ServerSettings {
            backup_dest: Some("s3://nope".to_string()),
            ..Default::default()
        },
    )
    .await;

    let run = schedule::run_once(&ctx, Trigger::Manual).await.unwrap();
    assert_eq!(run.outcome, Outcome::Failed);
    assert!(
        run.error.as_deref().unwrap_or_default().contains("unrecognized"),
        "unexpected error: {:?}",
        run.error
    );
}

#[tokio::test]
async fn a_self_destination_takes_a_snapshot_and_no_replica() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    save_settings(
        &ctx,
        &ServerSettings {
            backup_dest: Some("self".to_string()),
            ..Default::default()
        },
    )
    .await;

    let run = schedule::run_once(&ctx, Trigger::Manual).await.unwrap();
    assert_eq!(run.outcome, Outcome::Ok, "run failed: {:?}", run.error);
    assert_eq!(run.snapshot_id.as_deref(), Some(run.id.as_str()));

    // `global.json` is an entity, so the snapshot captured at least it.
    let snapshots = gitquarry_core::backup::snapshot::list(&ctx.backend(), &ctx.cipher)
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert!(snapshots[0].objects >= 1);
}

#[tokio::test]
async fn a_directory_destination_replicates_and_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    // A replica needs the source to have been bootstrapped, since that's the
    // object it copies first.
    gitquarry_core::crypto::bootstrap::get_or_create_cipher(ctx.backend().as_ref(), "test passphrase")
        .await
        .unwrap();
    save_settings(
        &ctx,
        &ServerSettings {
            backup_dest: Some(format!("path:{}", dest_dir.path().display())),
            ..Default::default()
        },
    )
    .await;

    let run = schedule::run_once(&ctx, Trigger::Cli).await.unwrap();
    assert_eq!(run.outcome, Outcome::Ok, "run failed: {:?}", run.error);

    let dest = gitquarry_test_support::backend(dest_dir.path());
    // The encryption config lands first, and the settings object rides along.
    for key in [
        gitquarry_core::crypto::bootstrap::ENCRYPTION_META_KEY,
        ".conf/global.json",
    ] {
        let (_, etag) = dest.get(key).await.unwrap();
        assert!(etag.is_some(), "{key} should have been replicated");
    }
}

#[tokio::test]
async fn the_snapshot_half_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    save_settings(
        &ctx,
        &ServerSettings {
            backup_dest: Some("self".to_string()),
            backup_snapshot: Some(false),
            ..Default::default()
        },
    )
    .await;

    let run = schedule::run_once(&ctx, Trigger::Manual).await.unwrap();
    assert_eq!(run.outcome, Outcome::Ok, "run failed: {:?}", run.error);
    assert_eq!(run.snapshot_id, None);
    assert!(
        gitquarry_core::backup::snapshot::list(&ctx.backend(), &ctx.cipher)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn snapshot_retention_applies_at_the_end_of_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    save_settings(
        &ctx,
        &ServerSettings {
            backup_dest: Some("self".to_string()),
            backup_keep: Some(1),
            ..Default::default()
        },
    )
    .await;

    for _ in 0..3 {
        let run = schedule::run_once(&ctx, Trigger::Cli).await.unwrap();
        assert_eq!(run.outcome, Outcome::Ok, "run failed: {:?}", run.error);
        std::thread::sleep(Duration::from_millis(2));
    }

    let snapshots = gitquarry_core::backup::snapshot::list(&ctx.backend(), &ctx.cipher)
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1, "only the newest snapshot should survive");
}

#[tokio::test]
async fn retention_is_off_unless_configured() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    save_settings(
        &ctx,
        &ServerSettings {
            backup_dest: Some("self".to_string()),
            ..Default::default()
        },
    )
    .await;

    for _ in 0..3 {
        schedule::run_once(&ctx, Trigger::Cli).await.unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }

    // Nothing set means keep everything: deleting a snapshot destroys the
    // only copy of what the live data used to be.
    let snapshots = gitquarry_core::backup::snapshot::list(&ctx.backend(), &ctx.cipher)
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 3);
}
