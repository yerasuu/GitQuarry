use gitquarry_federation::config::*;

#[test]
fn rejects_a_config_missing_the_passphrase() {
    let toml = r#"
            driver = "local"
            bucket = "/tmp/whatever"
            listen = "127.0.0.1:8181"
            core_internal_url = "http://127.0.0.1:8090/_internal"
            internal_token = "shh"
            passphrase = ""
        "#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CONFIG_NAME);
    std::fs::write(&path, toml).unwrap();
    let err = Config::load_from(&path).unwrap_err();
    assert!(err.to_string().contains("passphrase"));
}
