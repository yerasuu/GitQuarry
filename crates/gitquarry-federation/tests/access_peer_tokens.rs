use gitquarry_federation::access::peer_tokens::*;

#[test]
fn validate_peer_credential_rejects_empty_too_long_and_control_chars() {
    assert!(validate_peer_credential("token_abc123", "peer A").is_ok());
    assert!(validate_peer_credential("", "peer A").is_err());
    assert!(validate_peer_credential(&"x".repeat(MAX_SECRET_LEN + 1), "peer A").is_err());
    assert!(validate_peer_credential("bad\nsecret", "peer A").is_err());
    assert!(validate_peer_credential("token_abc123", &"x".repeat(MAX_PEER_LABEL_LEN + 1)).is_err());
}
