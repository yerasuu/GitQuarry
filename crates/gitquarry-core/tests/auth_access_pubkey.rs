use gitquarry_core::auth::access::{self, key_fingerprint};
use gitquarry_core::auth::entities::User;
use std::collections::BTreeMap;

const TEST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBI0nMr+cSaXUL1TPRVN35CkOEigmuVzAYqDCPMHnCcO alice@example.com";
const OTHER_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKgxCLGGh8p1cIfHWJmZ6r+d+I79H9d2c9jw2s0ac7iC bob@example.com";

/// Never in alice's `pubkeys`: stands in for a key she has since removed.
const REVOKED_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKHPqP+qQZkU0rnfN7Jurh960Xlp8dwuqnhz+tkrCm49 carol@example.com";

#[test]
fn fingerprint_ignores_comment() {
    let with_comment = key_fingerprint(TEST_KEY).unwrap();
    let without_comment =
        key_fingerprint("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBI0nMr+cSaXUL1TPRVN35CkOEigmuVzAYqDCPMHnCcO").unwrap();
    assert_eq!(with_comment, without_comment);
    assert!(with_comment.starts_with("SHA256:"));
}

#[tokio::test]
async fn a_claimed_fingerprint_resolves_only_while_the_user_still_holds_that_key() {
    let dir = tempfile::tempdir().unwrap();
    let backend = gitquarry_test_support::backend(dir.path());
    let cipher = gitquarry_test_support::cipher();
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
        is_federation_moderator: false,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        totp_last_step: 0,
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    };
    access::put_user(&*backend, &cipher, &user, None).await.unwrap();

    let held = key_fingerprint(OTHER_KEY).unwrap();
    assert!(access::claim_pubkey(&*backend, &cipher, &held, "alice").await.unwrap());
    let resolved = access::resolve_by_fingerprint(&*backend, &cipher, &held).await.unwrap();
    assert_eq!(resolved.map(|u| u.username).as_deref(), Some("alice"));

    // An index entry left pointing at alice for a key she no longer holds
    // (a revocation whose index cleanup never ran) must not log anyone in.
    let revoked = key_fingerprint(REVOKED_KEY).unwrap();
    assert!(
        access::claim_pubkey(&*backend, &cipher, &revoked, "alice")
            .await
            .unwrap()
    );
    assert!(
        access::resolve_by_fingerprint(&*backend, &cipher, &revoked)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        access::resolve_by_fingerprint(&*backend, &cipher, "SHA256:doesnotexist")
            .await
            .unwrap()
            .is_none()
    );
}
