use gitquarry_core::mirror::gc::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[test]
fn parses_age_units() {
    assert_eq!(parse_age("30s").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_age("5m").unwrap(), Duration::from_secs(300));
    assert_eq!(parse_age("2h").unwrap(), Duration::from_secs(7200));
    assert_eq!(parse_age("3d").unwrap(), Duration::from_secs(3 * 86400));
}

#[test]
fn parses_size_units() {
    assert_eq!(parse_size("100").unwrap(), 100);
    assert_eq!(parse_size("100B").unwrap(), 100);
    assert_eq!(parse_size("5K").unwrap(), 5 * 1024);
    assert_eq!(parse_size("512M").unwrap(), 512 * 1024 * 1024);
    assert_eq!(parse_size("5G").unwrap(), 5 * 1024 * 1024 * 1024);
    assert_eq!(parse_size("5g").unwrap(), 5 * 1024 * 1024 * 1024);
}

#[test]
fn rejects_bad_units_and_numbers() {
    assert!(parse_age("3x").is_err());
    assert!(parse_age("abc").is_err());
    assert!(parse_size("5T").is_err());
    assert!(parse_size("").is_err());
}

/// Builds an `AppContext` good enough for `sweep_once`: it only ever
/// touches `mirrors_root`/`mirror_max_age`/`mirror_max_size_bytes`/
/// `locks` on this path, never `backend`/`cipher`, so both are built
/// from local, no-network config just to satisfy the type.
fn dummy_ctx(
    mirrors_root: PathBuf,
    mirror_max_age: Duration,
    mirror_max_size_bytes: u64,
) -> gitquarry_core::app::AppContext {
    let cfg = gitquarry_core::config::BucketConfig {
        backend: gitquarry_core::config::BackendConfig::S3(gitquarry_core::config::S3Params {
            bucket: "test".to_string(),
            region: None,
            endpoint_url: None,
            access_key_id: "test".to_string(),
            secret_access_key: "test".to_string().into(),
            path_style: false,
            allow_insecure_http: false,
        }),
        passphrase: "test".to_string(),
    };
    gitquarry_core::app::AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root,
        config_path: PathBuf::new(),
        exe_path: PathBuf::new(),
        backend: gitquarry_core::storage::build_backend(&cfg).expect("building the test backend"),
        locks: gitquarry_core::app::Locks::default(),
        cipher: Arc::new(
            gitquarry_core::crypto::Cipher::derive("test", &[0u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
        ),
        mirror_max_age,
        mirror_max_size_bytes,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
    }
}

/// A fake bare-repo mirror: just enough for `list_mirrors` to recognize
/// it (a `HEAD` file) and for the age/size logic to have something to
/// work with (a marker backdated by `age_ago`, an optional padding file
/// for `size_bytes`).
fn make_fake_mirror(root: &Path, repo_name: &str, age_ago: Duration, size_bytes: u64) {
    let dir = root.join(repo_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    if size_bytes > 0 {
        std::fs::write(dir.join("pad.bin"), vec![0u8; size_bytes as usize]).unwrap();
    }
    let marker = dir.join(LAST_ACCESS_MARKER);
    std::fs::write(&marker, b"").unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&marker).unwrap();
    f.set_modified(SystemTime::now() - age_ago).unwrap();
}

#[tokio::test]
async fn sweep_evicts_stale_mirror_but_keeps_fresh_one() {
    let root = tempfile::tempdir().unwrap();
    make_fake_mirror(root.path(), "old-repo", Duration::from_secs(2 * 3600), 0);
    make_fake_mirror(root.path(), "team/fresh-repo", Duration::from_secs(1), 0);

    let ctx = dummy_ctx(root.path().to_path_buf(), Duration::from_secs(3600), u64::MAX);
    sweep_once(&ctx).await.unwrap();

    assert!(!root.path().join("old-repo").exists(), "stale mirror should be evicted");
    assert!(
        root.path().join("team/fresh-repo").exists(),
        "recently-queried mirror should survive"
    );
}

#[tokio::test]
async fn sweep_evicts_lru_when_over_size_cap() {
    let root = tempfile::tempdir().unwrap();
    // All well within max_age, so only the size cap decides. Oldest
    // access first == evicted first.
    make_fake_mirror(root.path(), "oldest", Duration::from_secs(30), 100);
    make_fake_mirror(root.path(), "middle", Duration::from_secs(20), 100);
    make_fake_mirror(root.path(), "newest", Duration::from_secs(10), 100);

    // Cap fits only the newest one (~100 bytes + HEAD); the other two
    // must go.
    let ctx = dummy_ctx(root.path().to_path_buf(), Duration::from_secs(3600), 150);
    sweep_once(&ctx).await.unwrap();

    assert!(!root.path().join("oldest").exists());
    assert!(!root.path().join("middle").exists());
    assert!(root.path().join("newest").exists());
}
