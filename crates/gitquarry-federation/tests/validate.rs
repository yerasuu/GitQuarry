use gitquarry_federation::validate::*;

#[test]
fn accepts_the_supported_schemes() {
    assert!(validate_web_base_url("https://acme.example").is_ok());
    assert!(validate_web_base_url("http://127.0.0.1:8080").is_ok());
    assert!(validate_git_base_url("https://acme.example").is_ok());
    assert!(validate_git_base_url("ssh://acme.example:2222").is_ok());
}

#[test]
fn rejects_empty_bad_scheme_and_whitespace() {
    assert!(validate_web_base_url("").is_err());
    assert!(validate_web_base_url("ftp://acme.example").is_err());
    assert!(validate_web_base_url("https:// acme.example").is_err());
    assert!(validate_web_base_url("https://").is_err());
}

#[test]
fn rejects_the_dropped_git_daemon_scheme() {
    assert!(validate_git_base_url("git://acme.example:9418").is_err());
    assert!(validate_web_base_url("git://acme.example:9418").is_err());
}

#[test]
fn rejects_a_non_http_web_base_url() {
    assert!(validate_web_base_url("ssh://acme.example:2222").is_err());
}
