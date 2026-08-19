use gitquarry_ci::config::Config;

fn write_config(dir: &std::path::Path, contents: &str) -> std::path::PathBuf {
    let path = dir.join("gitquarry-ci.toml");
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn load_from_rejects_a_missing_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        r#"
        driver = "local"
        bucket = "test"
        passphrase = ""
        listen = "127.0.0.1:9091"
        core_internal_url = "http://127.0.0.1:9090/_internal"
        internal_token = "shared-secret"
        "#,
    );
    assert!(Config::load_from(&path).is_err());
}

#[test]
fn load_from_rejects_a_missing_internal_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        r#"
        driver = "local"
        bucket = "test"
        passphrase = "secret"
        listen = "127.0.0.1:9091"
        core_internal_url = "http://127.0.0.1:9090/_internal"
        internal_token = ""
        "#,
    );
    assert!(Config::load_from(&path).is_err());
}

#[test]
fn load_from_accepts_a_valid_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        r#"
        driver = "local"
        bucket = "test"
        passphrase = "secret"
        listen = "127.0.0.1:9091"
        core_internal_url = "http://127.0.0.1:9090/_internal"
        internal_token = "shared-secret"
        "#,
    );
    let config = Config::load_from(&path).unwrap();
    assert_eq!(config.listen, "127.0.0.1:9091");
    assert_eq!(config.core_internal_url, "http://127.0.0.1:9090/_internal");
}
