use gitquarry_core::app::AppContext;
use gitquarry_core::backup::dest::Source;
use gitquarry_core::backup::inventory::{Class, Selection};
use gitquarry_core::backup::report::{self, Outcome, Trigger};
use gitquarry_core::backup::schedule::{self, DEFAULT_INTERVAL, MIN_INTERVAL};
use gitquarry_core::backup::snapshot::{self, Blobs};
use gitquarry_core::crypto::{Cipher, CompressionStrategy};
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::entities::ServerSettings;
use gitquarry_core::storage::settings::put_settings;
use gitquarry_test_support::ctx;
use std::sync::Arc;
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

#[tokio::test]
async fn blobs_in_the_snapshot_are_off_for_a_bucket_that_never_set_them_and_round_trip_once_set() {
    // A `global.json` written before the field existed must read as "off":
    // turning it on multiplies a backup's size.
    let legacy: ServerSettings = serde_json::from_str(r#"{"backup_snapshot":true}"#).unwrap();
    assert_eq!(legacy.backup_snapshot_blobs, None);
    assert!(serde_json::from_str::<ServerSettings>(r#"{"backup_snapshot_blobs":"yes"}"#).is_err());

    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    save_settings(
        &ctx,
        &ServerSettings {
            backup_snapshot_blobs: Some(true),
            ..Default::default()
        },
    )
    .await;
    let (stored, _) = gitquarry_core::storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher)
        .await
        .unwrap();
    assert_eq!(stored.backup_snapshot_blobs, Some(true));
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

/// Writes `body` the way `storage::remote::seal_and_upload` stores packs, LFS
/// objects and release files: `Cipher::seal_stream` at any size.
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

async fn exists(b: &Arc<dyn Backend>, key: &str) -> bool {
    b.get(key).await.unwrap().1.is_some()
}

fn patterned(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// Each blob class once under the snapshot's 1 MiB in-memory cut-off and once
/// over it.
fn blobs() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("repos/r1/objects/pack/pack-aa.pack", patterned(200)),
        ("repos/r1/objects/pack/pack-bb.pack", patterned((1 << 20) + 1)),
        ("repos/r1/lfs/small", patterned(100)),
        ("repos/r1/lfs/big", patterned(3 << 20)),
        ("repos/r1/releases/v1/assets/tool.tar.gz", patterned(300)),
        ("repos/r1/releases/v1/assets/tool.iso", patterned(2 << 20)),
    ]
}

async fn self_backup_with_blobs(ctx: &AppContext, blobs_setting: Option<bool>) {
    save_settings(
        ctx,
        &ServerSettings {
            backup_dest: Some("self".to_string()),
            backup_snapshot_blobs: blobs_setting,
            ..Default::default()
        },
    )
    .await;
    for (key, body) in blobs() {
        put_stream_sealed(&ctx.backend(), &ctx.cipher, key, &body).await;
    }
}

#[tokio::test]
async fn by_default_a_snapshot_holds_the_entities_and_no_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    self_backup_with_blobs(&ctx, None).await;

    let run = schedule::run_once(&ctx, Trigger::Manual).await.unwrap();
    assert_eq!(run.outcome, Outcome::Ok, "run failed: {:?}", run.error);
    let id = run.snapshot_id.expect("a snapshot");

    let manifests = snapshot::list(&ctx.backend(), &ctx.cipher).await.unwrap();
    assert_eq!(manifests[0].classes, ["entities"]);
    assert!(exists(&ctx.backend(), &format!(".conf/backups/{id}/.conf/global.json")).await);
    for (key, _) in blobs() {
        assert!(
            !exists(&ctx.backend(), &format!(".conf/backups/{id}/{key}")).await,
            "{key}"
        );
    }
}

#[tokio::test]
async fn with_the_setting_on_a_snapshot_holds_git_lfs_and_releases_and_restores_them_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let restored_dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    self_backup_with_blobs(&ctx, Some(true)).await;
    let meta_key = "repos/r1/releases/v1/meta.json";
    let refs_key = "repos/r1/refs.json";
    for key in [meta_key, refs_key] {
        let sealed = ctx.cipher.seal(key, b"{}", CompressionStrategy::None).unwrap();
        ctx.backend().put_cas(key, sealed, None).await.unwrap();
    }

    let run = schedule::run_once(&ctx, Trigger::Manual).await.unwrap();
    assert_eq!(run.outcome, Outcome::Ok, "run failed: {:?}", run.failures);
    let id = run.snapshot_id.expect("a snapshot");
    let manifests = snapshot::list(&ctx.backend(), &ctx.cipher).await.unwrap();
    assert_eq!(manifests[0].classes, ["entities", "git", "lfs", "releases"]);

    let restored = gitquarry_test_support::backend(restored_dir.path());
    let report = snapshot::restore(&ctx.backend(), &ctx.cipher, restored.as_ref(), &ctx.cipher, &id, false)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "restore failures: {:?}", report.failures);
    for (key, body) in blobs() {
        let (sealed, _) = restored.get(key).await.unwrap();
        assert!(Cipher::is_stream_sealed(&sealed), "{key} changed envelope");
        assert_eq!(read_stream_sealed(&restored, &ctx.cipher, key).await, body, "{key}");
    }
    for key in [meta_key, refs_key] {
        let (sealed, _) = restored.get(key).await.unwrap();
        assert_eq!(ctx.cipher.open(key, &sealed).unwrap(), b"{}", "{key}");
    }
}

#[tokio::test]
async fn the_cli_flag_overrides_the_setting_for_one_run() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    self_backup_with_blobs(&ctx, None).await;
    let backend = ctx.backend();
    let source = Source {
        bucket_name: &ctx.bucket,
        backend: &backend,
        cipher: &ctx.cipher,
        config_path: &ctx.config_path,
    };

    let on = schedule::run_for(&source, Trigger::Cli, None, None, Some(Blobs::Capture))
        .await
        .unwrap();
    assert_eq!(on.outcome, Outcome::Ok, "run failed: {:?}", on.failures);
    let pack = blobs()[0].0;
    let on_id = on.snapshot_id.expect("a snapshot");
    assert!(exists(&backend, &format!(".conf/backups/{on_id}/{pack}")).await);

    std::thread::sleep(Duration::from_millis(2));
    self_backup_with_blobs(&ctx, Some(true)).await;
    let off = schedule::run_for(&source, Trigger::Cli, None, None, Some(Blobs::Skip))
        .await
        .unwrap();
    let off_id = off.snapshot_id.expect("a snapshot");
    assert!(!exists(&backend, &format!(".conf/backups/{off_id}/{pack}")).await);
}

#[test]
fn a_class_switched_off_stays_out_of_the_snapshot_even_with_blobs_on() {
    let selection = Selection {
        lfs: false,
        entities: false,
        ..Selection::default()
    };
    assert_eq!(
        snapshot::classes(&selection, Blobs::Capture),
        [Class::Git, Class::Releases]
    );
    assert!(snapshot::classes(&selection, Blobs::Skip).is_empty());
}

#[tokio::test]
async fn a_corrupted_blob_at_the_source_fails_the_run_and_is_never_captured() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    self_backup_with_blobs(&ctx, Some(true)).await;
    let bad = blobs()[1].0;
    let (mut sealed, etag) = ctx.backend().get(bad).await.unwrap();
    *sealed.last_mut().unwrap() ^= 0x01;
    ctx.backend().put_cas(bad, sealed, etag.as_deref()).await.unwrap();

    let run = schedule::run_once(&ctx, Trigger::Manual).await.unwrap();
    assert_eq!(run.outcome, Outcome::Partial);
    assert_eq!(run.failures_total, 1, "{:?}", run.failures);
    assert!(run.failures[0].starts_with(bad), "{:?}", run.failures);
    let id = run.snapshot_id.expect("a snapshot");
    assert!(!exists(&ctx.backend(), &format!(".conf/backups/{id}/{bad}")).await);
    let manifests = snapshot::list(&ctx.backend(), &ctx.cipher).await.unwrap();
    assert!(!manifests[0].complete);
}
