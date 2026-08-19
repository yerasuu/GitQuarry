use gitquarry_core::config::*;

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
    let raw: RawConfig = toml::from_str(toml).unwrap();
    let cfg = raw.buckets.get("main").unwrap();
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
    let raw: RawConfig = toml::from_str(toml).unwrap();
    let cfg = raw.buckets.get("cache").unwrap();
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
    assert!(toml::from_str::<RawConfig>(toml).is_err());
}
