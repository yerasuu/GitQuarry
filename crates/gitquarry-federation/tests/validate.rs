use gitquarry_federation::validate::*;

#[test]
fn accepts_the_supported_schemes() {
    assert!(validate_instance_url("https://acme.example").is_ok());
    assert!(validate_instance_url("http://127.0.0.1:8080").is_ok());
    assert!(validate_instance_url("ssh://acme.example:2222").is_ok());
}

#[test]
fn rejects_empty_bad_scheme_and_whitespace() {
    assert!(validate_instance_url("").is_err());
    assert!(validate_instance_url("ftp://acme.example").is_err());
    assert!(validate_instance_url("https:// acme.example").is_err());
    assert!(validate_instance_url("https://").is_err());
}
