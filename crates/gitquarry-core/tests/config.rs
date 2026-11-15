use gitquarry_core::config::*;

fn load(toml: &str) -> anyhow::Result<Config> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gitquarry.toml");
    std::fs::write(&path, toml).unwrap();
    Config::load_from(&path)
}

#[test]
fn parses_an_s3_bucket_with_driver_tag() {
    let toml = r#"
        [buckets.main]
        driver = "s3"
        bucket = "my-bucket"
        region = "us-east-1"
        access_key_id = "AKIA"
        secret_access_key = "secret"
        passphrase = "hunter2"
    "#;
    let config = load(toml).unwrap();
    let cfg = config.bucket("main").unwrap();
    assert_eq!(cfg.passphrase, "hunter2");
    let BackendConfig::S3(params) = &cfg.backend else {
        panic!("expected driver s3");
    };
    assert_eq!(params.bucket, "my-bucket");
    assert_eq!(params.region.as_deref(), Some("us-east-1"));
}

#[test]
fn parses_an_r2_bucket_deriving_endpoint_from_account_id() {
    let toml = r#"
        [buckets.cache]
        driver = "r2"
        bucket = "my-bucket"
        account_id = "abc123"
        access_key_id = "AKIA"
        secret_access_key = "secret"
        passphrase = "hunter2"
    "#;
    let config = load(toml).unwrap();
    let cfg = config.bucket("cache").unwrap();
    let BackendConfig::R2(params) = &cfg.backend else {
        panic!("expected driver r2");
    };
    assert_eq!(params.account_id, "abc123");
}

#[test]
fn rejects_a_bucket_missing_the_driver_tag() {
    // No `driver = "..."`: same "hard error over silent fallback"
    // stance as a missing `passphrase` — this used to be a valid
    // pre-driver config, so an existing gitquarry.toml needs the
    // one-line edit rather than getting silently guessed at.
    let toml = r#"
        [buckets.main]
        bucket = "my-bucket"
        access_key_id = "AKIA"
        secret_access_key = "secret"
        passphrase = "hunter2"
    "#;
    assert!(load(toml).is_err());
}

/// Two buckets is the normal shape once one exists purely as a backup
/// destination or a mirror target, so the served one has to be named.
#[test]
fn default_bucket_picks_the_served_one_among_several() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gitquarry.toml");
    std::fs::write(
        &path,
        r#"
        default_bucket = "main"

        [buckets.main]
        driver = "local"
        bucket = "/tmp/main"
        passphrase = "hunter2"

        [buckets.offsite]
        driver = "local"
        bucket = "/tmp/offsite"
        passphrase = "hunter2"
    "#,
    )
    .unwrap();
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.default_bucket_name(), Some("main"));
}

#[test]
fn several_buckets_without_a_default_leave_the_choice_to_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gitquarry.toml");
    std::fs::write(
        &path,
        r#"
        [buckets.main]
        driver = "local"
        bucket = "/tmp/main"
        passphrase = "hunter2"

        [buckets.offsite]
        driver = "local"
        bucket = "/tmp/offsite"
        passphrase = "hunter2"
    "#,
    )
    .unwrap();
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.default_bucket_name(), None);
}

#[test]
fn a_single_bucket_still_needs_no_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gitquarry.toml");
    std::fs::write(
        &path,
        r#"
        [buckets.main]
        driver = "local"
        bucket = "/tmp/main"
        passphrase = "hunter2"
    "#,
    )
    .unwrap();
    let cfg = Config::load_from(&path).unwrap();
    assert_eq!(cfg.default_bucket_name(), Some("main"));
}

/// A typo'd default would otherwise only surface at startup, as the far
/// less obvious "bucket 'mian' not configured".
#[test]
fn rejects_a_default_bucket_naming_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gitquarry.toml");
    std::fs::write(
        &path,
        r#"
        default_bucket = "mian"

        [buckets.main]
        driver = "local"
        bucket = "/tmp/main"
        passphrase = "hunter2"
    "#,
    )
    .unwrap();
    let err = Config::load_from(&path).unwrap_err().to_string();
    assert!(err.contains("default_bucket"), "unexpected error: {err}");
}

/// s3db's relative `./local` default would follow each process's cwd (a git
/// hook runs inside the mirror) and be shared by every bucket in the file;
/// each bucket must get its own fixed spool directory instead.
#[test]
fn each_bucket_gets_its_own_absolute_spool_dir_by_default() {
    let toml = r#"
        [buckets.main]
        driver = "local"
        bucket = "/srv/main"
        passphrase = "hunter2"

        [buckets.backup]
        driver = "local"
        bucket = "/srv/backup"
        passphrase = "hunter2"
    "#;
    let config = load(toml).unwrap();
    let main = &config.bucket("main").unwrap().resilience.local_dir;
    let backup = &config.bucket("backup").unwrap().resilience.local_dir;
    assert!(main.is_absolute(), "{}", main.display());
    assert_ne!(main, backup);
}

#[test]
fn a_relative_spool_dir_resolves_against_the_config_file() {
    let config_path = std::path::Path::new("/etc/gitquarry/gitquarry.toml");
    assert_eq!(
        resolve_local_dir("main", std::path::Path::new("state"), config_path),
        std::path::Path::new("/etc/gitquarry/state")
    );
    assert_eq!(
        resolve_local_dir("main", std::path::Path::new("/var/lib/gq"), config_path),
        std::path::Path::new("/var/lib/gq")
    );
}

#[test]
fn rejects_a_tracker_section_with_a_blank_internal_token() {
    for token in ["", "   "] {
        let toml = format!(
            r#"
            [buckets.main]
            driver = "s3"
            bucket = "my-bucket"
            access_key_id = "AKIA"
            secret_access_key = "secret"
            passphrase = "hunter2"

            [tracker]
            internal_bind = "127.0.0.1:8091"
            internal_token = "{token}"
            tracker_url = "http://127.0.0.1:8092"
            "#
        );
        let err = load(&toml).unwrap_err();
        assert!(err.to_string().contains("internal_token"), "{err}");
    }
}

#[test]
fn rejects_a_federation_section_with_a_blank_internal_token() {
    for token in ["", "   "] {
        let toml = format!(
            r#"
            [buckets.main]
            driver = "s3"
            bucket = "my-bucket"
            access_key_id = "AKIA"
            secret_access_key = "secret"
            passphrase = "hunter2"

            [federation]
            internal_bind = "127.0.0.1:8090"
            internal_token = "{token}"
            federation_url = "http://127.0.0.1:8181"
            "#
        );
        let err = load(&toml).unwrap_err();
        assert!(err.to_string().contains("internal_token"), "{err}");
    }
}

const ONE_BUCKET: &str = r#"
    [buckets.main]
    driver = "local"
    bucket = "/tmp/gq"
    passphrase = "hunter2"
"#;

#[test]
fn a_config_without_an_api_section_keeps_the_api_defaults() {
    let config = load(ONE_BUCKET).unwrap();
    assert_eq!(config.api().requests_per_minute, 120);
    assert_eq!(config.api().failed_auth_per_minute, 30);
}

#[test]
fn the_api_section_lowers_the_limits() {
    let config = load(&format!("{ONE_BUCKET}\n[api]\nrequests_per_minute = 10\n")).unwrap();
    assert_eq!(config.api().requests_per_minute, 10);
    assert_eq!(config.api().failed_auth_per_minute, 30);
}

#[test]
fn rejects_api_limits_outside_their_range_or_unknown_keys() {
    for section in [
        "requests_per_minute = 0",
        "requests_per_minute = 1201",
        "failed_auth_per_minute = 301",
        "request_per_minute = 10",
    ] {
        assert!(load(&format!("{ONE_BUCKET}\n[api]\n{section}\n")).is_err(), "{section}");
    }
}
