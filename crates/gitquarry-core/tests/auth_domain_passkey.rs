//! `auth::domain::passkey` over a real bucket, fed by the shared software
//! authenticator.

use gitquarry_auth_crypto::webauthn::{self, RelyingParty, WebauthnError};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::passkey::{self, Assertion, PasskeyRefusal};
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::User;
use gitquarry_i18n::Locale;
use gitquarry_test_support::authenticator::{Authenticator, Key, UP, UV};
use gitquarry_test_support::{account, ctx};

fn rp() -> RelyingParty {
    RelyingParty::from_public_url("https://git.example.com").unwrap()
}

/// `authenticator` registered for `owner`, verified the way the web layer
/// verifies it.
async fn enroll(ctx: &AppContext, owner: &User, authenticator: &Authenticator, label: &str) -> anyhow::Result<()> {
    let challenge = webauthn::new_challenge();
    let reg = authenticator.register(&challenge, UP | UV);
    let credential =
        webauthn::verify_registration(&rp(), &challenge, &reg.client_data_json, &reg.attestation_object, true).unwrap();
    passkey::register(ctx, owner, credential, label).await.map(drop)
}

fn assertion(
    authenticator: &Authenticator,
    challenge: &str,
    flags: u8,
    counter: u32,
    owner: Option<&User>,
) -> Assertion {
    let parts = authenticator.assert(challenge, flags, counter);
    Assertion {
        credential_id: webauthn::encode_id(&authenticator.credential_id),
        client_data_json: parts.client_data_json,
        authenticator_data: parts.authenticator_data,
        signature: parts.signature,
        user_handle: owner.map(|u| u.uid.as_str().as_bytes().to_vec()),
    }
}

async fn sign_in(
    ctx: &AppContext,
    authenticator: &Authenticator,
    counter: u32,
    expected_owner: Option<&User>,
) -> Result<User, PasskeyRefusal> {
    let challenge = webauthn::new_challenge();
    let a = assertion(authenticator, &challenge, UP | UV, counter, None);
    passkey::verify(ctx, &rp(), &challenge, &a, expected_owner.map(|u| &u.uid), true)
        .await
        .unwrap()
        .map(|(owner, _)| owner)
}

#[tokio::test]
async fn a_registered_passkey_signs_its_owner_in_and_records_its_use() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let key = Authenticator::new(Key::p256(1), b"alice-key", &rp());
    enroll(&ctx, &alice, &key, "laptop").await.unwrap();

    let listed = passkey::list(&ctx, &alice.uid).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].label, "laptop");
    assert!(listed[0].last_used_at.is_none());

    assert_eq!(sign_in(&ctx, &key, 3, None).await.unwrap().username, "alice");
    let after = &passkey::list(&ctx, &alice.uid).await.unwrap()[0];
    assert_eq!(after.sign_count, 3);
    assert!(after.last_used_at.is_some());
}

#[tokio::test]
async fn a_counting_authenticator_replayed_at_the_same_count_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let key = Authenticator::new(Key::ed25519(2), b"alice-key", &rp());
    enroll(&ctx, &alice, &key, "yubikey").await.unwrap();

    sign_in(&ctx, &key, 7, None).await.unwrap();
    // A clone of the key signing with the counter it had when copied.
    assert_eq!(
        sign_in(&ctx, &key, 7, None).await.unwrap_err(),
        PasskeyRefusal::Webauthn(WebauthnError::CounterRegressed)
    );
    assert_eq!(passkey::list(&ctx, &alice.uid).await.unwrap()[0].sign_count, 7);
}

#[tokio::test]
async fn a_second_factor_accepts_only_the_passkey_of_the_account_that_passed_its_password() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let bob = account(&ctx, "bob", None).await;
    let bobs_key = Authenticator::new(Key::p256(3), b"bob-key", &rp());
    enroll(&ctx, &bob, &bobs_key, "phone").await.unwrap();

    assert_eq!(
        sign_in(&ctx, &bobs_key, 0, Some(&alice)).await.unwrap_err(),
        PasskeyRefusal::NotThisAccount
    );
    assert_eq!(sign_in(&ctx, &bobs_key, 0, Some(&bob)).await.unwrap().username, "bob");
}

#[tokio::test]
async fn a_user_handle_naming_someone_else_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let bob = account(&ctx, "bob", None).await;
    let key = Authenticator::new(Key::p256(1), b"alice-key", &rp());
    enroll(&ctx, &alice, &key, "laptop").await.unwrap();

    let challenge = webauthn::new_challenge();
    let forged = assertion(&key, &challenge, UP | UV, 0, Some(&bob));
    let outcome = passkey::verify(&ctx, &rp(), &challenge, &forged, None, true)
        .await
        .unwrap();
    assert_eq!(outcome.unwrap_err(), PasskeyRefusal::NotThisAccount);

    let honest = assertion(&key, &challenge, UP | UV, 0, Some(&alice));
    assert!(
        passkey::verify(&ctx, &rp(), &challenge, &honest, None, true)
            .await
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
async fn a_credential_id_can_be_registered_only_once_across_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let bob = account(&ctx, "bob", None).await;
    let key = Authenticator::new(Key::p256(1), b"shared-id", &rp());
    enroll(&ctx, &alice, &key, "laptop").await.unwrap();

    let err = enroll(&ctx, &bob, &key, "stolen").await.unwrap_err();
    assert!(err.to_string().contains("already registered"), "{err}");
    assert!(passkey::list(&ctx, &bob.uid).await.unwrap().is_empty());
    assert_eq!(sign_in(&ctx, &key, 0, None).await.unwrap().username, "alice");
}

#[tokio::test]
async fn labels_are_validated_and_accounts_capped() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    for bad in ["", "   ", "tab\there", &"x".repeat(65)] {
        let key = Authenticator::new(Key::p256(1), b"label-case", &rp());
        assert!(enroll(&ctx, &alice, &key, bad).await.is_err(), "{bad:?} accepted");
    }
    for n in 0..gitquarry_core::auth::access::MAX_PASSKEYS_PER_USER {
        let key = Authenticator::new(Key::p256(1), format!("key-{n}").as_bytes(), &rp());
        enroll(&ctx, &alice, &key, "k").await.unwrap();
    }
    let one_more = Authenticator::new(Key::p256(1), b"one-too-many", &rp());
    assert!(enroll(&ctx, &alice, &one_more, "k").await.is_err());
}

#[tokio::test]
async fn only_the_owner_removes_a_passkey_and_a_removed_one_no_longer_signs_in() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let bob = account(&ctx, "bob", None).await;
    let key = Authenticator::new(Key::p256(1), b"alice-key", &rp());
    enroll(&ctx, &alice, &key, "laptop").await.unwrap();
    let stored = passkey::list(&ctx, &alice.uid).await.unwrap().remove(0);

    let epoch = |name: &'static str| {
        let ctx = &ctx;
        async move {
            gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, name)
                .await
                .unwrap()
                .0
                .unwrap()
                .session_epoch
        }
    };
    let (alice_before, bob_before) = (epoch("alice").await, epoch("bob").await);

    assert!(!passkey::remove(&ctx, &bob, &stored.key, Locale::DEFAULT).await.unwrap());
    assert!(sign_in(&ctx, &key, 0, None).await.is_ok());
    assert_eq!(epoch("bob").await, bob_before);

    assert!(
        passkey::remove(&ctx, &alice, &stored.key, Locale::DEFAULT)
            .await
            .unwrap()
    );
    assert!(passkey::list(&ctx, &alice.uid).await.unwrap().is_empty());
    // Alice's open web sessions die with the passkey.
    assert_ne!(epoch("alice").await, alice_before);
    assert_eq!(
        sign_in(&ctx, &key, 0, None).await.unwrap_err(),
        PasskeyRefusal::UnknownCredential
    );
}

#[tokio::test]
async fn deleting_the_account_takes_its_passkeys_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let key = Authenticator::new(Key::p256(1), b"alice-key", &rp());
    enroll(&ctx, &alice, &key, "laptop").await.unwrap();

    user::delete(&ctx, "alice").await.unwrap();
    assert_eq!(
        sign_in(&ctx, &key, 0, None).await.unwrap_err(),
        PasskeyRefusal::UnknownCredential
    );
}

#[tokio::test]
async fn a_passkey_only_sign_in_needs_user_verification() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let alice = account(&ctx, "alice", None).await;
    let key = Authenticator::new(Key::p256(1), b"alice-key", &rp());
    enroll(&ctx, &alice, &key, "laptop").await.unwrap();

    let challenge = webauthn::new_challenge();
    let present_only = assertion(&key, &challenge, UP, 0, None);
    let outcome = passkey::verify(&ctx, &rp(), &challenge, &present_only, None, true)
        .await
        .unwrap();
    assert_eq!(
        outcome.unwrap_err(),
        PasskeyRefusal::Webauthn(WebauthnError::UserNotVerified)
    );
    // As the second factor after a password, presence is enough.
    let outcome = passkey::verify(&ctx, &rp(), &challenge, &present_only, Some(&alice.uid), false)
        .await
        .unwrap();
    assert!(outcome.is_ok());
}
