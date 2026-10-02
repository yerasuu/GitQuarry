use gitquarry_federation::access::*;

#[test]
fn validate_token_label_rejects_empty_too_long_and_control_chars() {
    assert!(validate_token_label("peer A - browsing").is_ok());
    assert!(validate_token_label("").is_err());
    assert!(validate_token_label(&"x".repeat(128)).is_ok());
    assert!(validate_token_label(&"x".repeat(129)).is_err());
    assert!(validate_token_label("bad\nlabel").is_err());
}

#[test]
fn generate_secret_is_prefixed_and_unique() {
    let a = generate_secret();
    let b = generate_secret();
    assert!(a.starts_with("token_"));
    assert_eq!(a.len(), "token_".len() + 40);
    assert_ne!(a, b);
}

#[test]
fn hash_token_is_deterministic_and_sensitive_to_input() {
    let secret = generate_secret();
    assert_eq!(hash_token(&secret), hash_token(&secret));
    assert_ne!(hash_token(&secret), hash_token(&generate_secret()));
}
