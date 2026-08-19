use gitquarry_core::auth::access::pubkey::*;
use gitquarry_core::auth::entities::User;
use std::collections::BTreeMap;

const TEST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBI0nMr+cSaXUL1TPRVN35CkOEigmuVzAYqDCPMHnCcO alice@example.com";
const OTHER_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKgxCLGGh8p1cIfHWJmZ6r+d+I79H9d2c9jw2s0ac7iC bob@example.com";

#[test]
fn fingerprint_ignores_comment() {
    let with_comment = key_fingerprint(TEST_KEY).unwrap();
    let without_comment =
        key_fingerprint("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBI0nMr+cSaXUL1TPRVN35CkOEigmuVzAYqDCPMHnCcO").unwrap();
    assert_eq!(with_comment, without_comment);
    assert!(with_comment.starts_with("SHA256:"));
}

#[test]
fn user_matches_any_of_its_own_pubkeys_by_fingerprint() {
    let user = User {
        uid: Default::default(),
        username: "alice".to_string(),
        email: None,
        pubkeys: vec![TEST_KEY.to_string(), OTHER_KEY.to_string()],
        gpg_keys: Vec::new(),
        repos: BTreeMap::new(),
        default_permission: None,
        password_hash: None,
        is_admin: false,
        session_epoch: 0,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    };
    let fp = key_fingerprint(OTHER_KEY).unwrap();
    assert!(user.fingerprints().any(|f| f == fp));
    assert!(!user.fingerprints().any(|f| f == "SHA256:doesnotexist"));
}
