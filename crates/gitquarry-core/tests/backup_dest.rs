use gitquarry_core::backup::dest::{self, Dest, Source};
use gitquarry_core::config::{Config, LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use s3db::LocalBackend;
use s3db_crypto::SALT_LEN;
use std::sync::Arc;

#[test]
fn every_destination_form_round_trips() {
    for raw in ["self", "bucket:offsite", "path:/srv/backup"] {
        let parsed = Dest::parse(raw).unwrap();
        assert_eq!(parsed.to_string(), raw, "{raw} should round-trip");
    }
}

#[test]
fn whitespace_is_tolerated() {
    assert_eq!(Dest::parse("  self  ").unwrap(), Dest::SelfBucket);
    assert_eq!(
        Dest::parse("bucket:  offsite ").unwrap(),
        Dest::Bucket("offsite".to_string())
    );
}

#[test]
fn a_relative_path_destination_is_refused() {
    let err = Dest::parse("path:backups").expect_err("a relative path must be refused");
    assert!(format!("{err:#}").contains("absolute"), "unexpected error: {err:#}");
}

#[test]
fn an_empty_bucket_name_is_refused() {
    assert!(Dest::parse("bucket:").is_err());
}

#[test]
fn an_unrecognized_form_is_refused() {
    let err = Dest::parse("s3://somewhere").expect_err("an unknown scheme must be refused");
    assert!(format!("{err:#}").contains("unrecognized"), "unexpected error: {err:#}");
}

#[test]
fn only_the_same_bucket_form_rules_out_a_replica() {
    assert!(!Dest::SelfBucket.supports_replica());
    assert!(Dest::Bucket("offsite".to_string()).supports_replica());
    assert!(Dest::Path("/srv/backup".into()).supports_replica());
}

fn write_config(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("gitquarry.toml");
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn a_bucket_that_is_not_configured_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "[buckets.live]\ndriver = \"local\"\nbucket = \"/tmp/live\"\npassphrase = \"pw\"\n",
    );
    let config = Config::load_from(&path).unwrap();

    let err = Dest::parse("bucket:typo")
        .unwrap()
        .validate_against(&config, "live")
        .expect_err("a bucket absent from gitquarry.toml must fail at save time");
    assert!(
        format!("{err:#}").contains("not configured"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn backing_a_bucket_up_into_itself_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "[buckets.live]\ndriver = \"local\"\nbucket = \"/tmp/live\"\npassphrase = \"pw\"\n",
    );
    let config = Config::load_from(&path).unwrap();

    let err = Dest::parse("bucket:live")
        .unwrap()
        .validate_against(&config, "live")
        .expect_err("naming the source bucket as the destination must be refused");
    assert!(
        format!("{err:#}").contains("already serving"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn a_configured_bucket_passes_validation() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "[buckets.live]\ndriver = \"local\"\nbucket = \"/tmp/live\"\npassphrase = \"pw\"\n\
         [buckets.offsite]\ndriver = \"local\"\nbucket = \"/tmp/offsite\"\npassphrase = \"pw\"\n",
    );
    let config = Config::load_from(&path).unwrap();
    Dest::parse("bucket:offsite")
        .unwrap()
        .validate_against(&config, "live")
        .unwrap();
}

#[test]
fn a_missing_directory_fails_validation() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "[buckets.live]\ndriver = \"local\"\nbucket = \"/tmp/live\"\npassphrase = \"pw\"\n",
    );
    let config = Config::load_from(&path).unwrap();
    let missing = dir.path().join("not-there");

    let err = Dest::parse(&format!("path:{}", missing.display()))
        .unwrap()
        .validate_against(&config, "live")
        .expect_err("a directory that doesn't exist must fail at save time");
    assert!(
        format!("{err:#}").contains("isn't a directory"),
        "unexpected error: {err:#}"
    );
}

fn source_parts(root: &std::path::Path) -> (Arc<dyn Backend>, Arc<Cipher>) {
    let backend: Arc<dyn Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("live passphrase", &[3u8; SALT_LEN]).unwrap());
    (backend, cipher)
}

#[tokio::test]
async fn resolving_self_reuses_the_live_backend_and_rules_out_a_replica() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_config(
        dir.path(),
        "[buckets.live]\ndriver = \"local\"\nbucket = \"/tmp/live\"\npassphrase = \"pw\"\n",
    );
    let (backend, cipher) = source_parts(dir.path());
    let source = Source {
        bucket_name: "live",
        backend: &backend,
        cipher: &cipher,
        config_path: &config_path,
    };

    let resolved = dest::resolve(&Dest::SelfBucket, &source).await.unwrap();
    assert!(!resolved.replica);
    // Nothing to verify: there's no second passphrase in play.
    resolved.verify_openable().await.unwrap();
}

#[tokio::test]
async fn resolving_a_directory_gives_a_writable_replica_target() {
    let dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let config_path = write_config(
        dir.path(),
        "[buckets.live]\ndriver = \"local\"\nbucket = \"/tmp/live\"\npassphrase = \"pw\"\n",
    );
    let (backend, cipher) = source_parts(dir.path());
    let source = Source {
        bucket_name: "live",
        backend: &backend,
        cipher: &cipher,
        config_path: &config_path,
    };

    let parsed = Dest::parse(&format!("path:{}", dest_dir.path().display())).unwrap();
    let resolved = dest::resolve(&parsed, &source).await.unwrap();
    assert!(resolved.replica);

    // Really writable, and readable back under the source's own cipher --
    // which is the point: a replica shares the source's data key.
    resolved
        .backend
        .put_cas("probe.json", b"hello".to_vec(), None)
        .await
        .unwrap();
    let (bytes, etag) = resolved.backend.get("probe.json").await.unwrap();
    assert!(etag.is_some());
    assert_eq!(bytes, b"hello");
}

#[tokio::test]
async fn resolving_a_bucket_that_is_not_configured_fails() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_config(
        dir.path(),
        "[buckets.live]\ndriver = \"local\"\nbucket = \"/tmp/live\"\npassphrase = \"pw\"\n",
    );
    let (backend, cipher) = source_parts(dir.path());
    let source = Source {
        bucket_name: "live",
        backend: &backend,
        cipher: &cipher,
        config_path: &config_path,
    };

    assert!(
        dest::resolve(&Dest::Bucket("nope".to_string()), &source).await.is_err(),
        "an unconfigured destination bucket must fail to resolve"
    );
}

#[tokio::test]
async fn a_destination_bucket_with_a_different_passphrase_is_reported() {
    let live_dir = tempfile::tempdir().unwrap();
    let offsite_dir = tempfile::tempdir().unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();
    let config_path = write_config(
        cfg_dir.path(),
        &format!(
            "[buckets.live]\ndriver = \"local\"\nbucket = \"{}\"\npassphrase = \"live pw\"\n\
             [buckets.offsite]\ndriver = \"local\"\nbucket = \"{}\"\npassphrase = \"a different pw\"\n",
            live_dir.path().display(),
            offsite_dir.path().display()
        ),
    );
    let (backend, cipher) = source_parts(live_dir.path());
    let source = Source {
        bucket_name: "live",
        backend: &backend,
        cipher: &cipher,
        config_path: &config_path,
    };

    // Bootstrap the live bucket under its own passphrase, then hand the
    // destination that same wrapped key -- exactly what a replica does.
    let live = gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), "live pw")
        .await
        .unwrap();
    drop(live);
    let resolved = dest::resolve(&Dest::Bucket("offsite".to_string()), &source)
        .await
        .unwrap();
    let (meta, _) = backend
        .get(gitquarry_core::crypto::bootstrap::ENCRYPTION_META_KEY)
        .await
        .unwrap();
    resolved
        .backend
        .put_cas(gitquarry_core::crypto::bootstrap::ENCRYPTION_META_KEY, meta, None)
        .await
        .unwrap();

    // The destination is configured with a passphrase that can't open the
    // key it now holds: a backup that exists and can't be read, which is
    // exactly what this check exists to refuse.
    let err = resolved
        .verify_openable()
        .await
        .expect_err("a mismatched destination passphrase must be reported");
    assert!(
        format!("{err:#}").contains("same passphrase"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn a_destination_bucket_with_the_same_passphrase_verifies() {
    let live_dir = tempfile::tempdir().unwrap();
    let offsite_dir = tempfile::tempdir().unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();
    let config_path = write_config(
        cfg_dir.path(),
        &format!(
            "[buckets.live]\ndriver = \"local\"\nbucket = \"{}\"\npassphrase = \"shared pw\"\n\
             [buckets.offsite]\ndriver = \"local\"\nbucket = \"{}\"\npassphrase = \"shared pw\"\n",
            live_dir.path().display(),
            offsite_dir.path().display()
        ),
    );
    let (backend, cipher) = source_parts(live_dir.path());
    let source = Source {
        bucket_name: "live",
        backend: &backend,
        cipher: &cipher,
        config_path: &config_path,
    };

    gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), "shared pw")
        .await
        .unwrap();
    let resolved = dest::resolve(&Dest::Bucket("offsite".to_string()), &source)
        .await
        .unwrap();
    let (meta, _) = backend
        .get(gitquarry_core::crypto::bootstrap::ENCRYPTION_META_KEY)
        .await
        .unwrap();
    resolved
        .backend
        .put_cas(gitquarry_core::crypto::bootstrap::ENCRYPTION_META_KEY, meta, None)
        .await
        .unwrap();

    resolved.verify_openable().await.unwrap();
}
