use gitquarry_federation::access::*;

#[test]
fn validate_peer_name_rejects_empty_control_and_separators() {
    assert!(validate_peer_name("acme-instance").is_ok());
    assert!(validate_peer_name("").is_err());
    assert!(validate_peer_name("bad\nname").is_err());
    assert!(validate_peer_name("acme/proj").is_err());
}
