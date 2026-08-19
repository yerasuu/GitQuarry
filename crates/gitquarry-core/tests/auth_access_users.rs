// `users` itself is a private submodule of `auth::access` (see that
// module's doc comment); its public items are re-exported at
// `auth::access::*`, which is the path visible from outside the crate.
use gitquarry_core::auth::access::*;

#[test]
fn validate_username_rejects_empty_and_control_chars() {
    assert!(validate_username("alice").is_ok());
    assert!(validate_username("").is_err());
    assert!(validate_username("  ").is_err());
    assert!(validate_username("bad\nname").is_err());
}

#[test]
fn validate_username_rejects_path_separators() {
    // Usernames are literal S3 key segments now (`.conf/users/<name>.json`).
    assert!(validate_username("acme/proj").is_err());
    assert!(validate_username("acme\\proj").is_err());
}
