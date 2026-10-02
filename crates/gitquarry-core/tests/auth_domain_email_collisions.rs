//! What happens when the same address ends up on both sides of the
//! primary/secondary split. The two live in separate indexes
//! (`EMAIL_INDEX`, claimed by the `#[entity(index)]` derive, and
//! `SECONDARY_EMAIL_INDEX`, hand-rolled), and only one of the two claim
//! paths consults the other — so the pair can drift into states where an
//! address is listed twice, or listed but claimed by nobody.

use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::user;
use gitquarry_i18n::Locale;
use gitquarry_test_support::account;

/// Adding your own primary address as a secondary must not silently store
/// it twice: `claim_secondary_email` returns `Ok(true)` when the address
/// already resolves to *you*, which the caller reads as "claimed" even
/// though no secondary index entry was written.
#[tokio::test]
async fn own_primary_address_is_refused_as_a_secondary() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", Some("alice@example.com")).await;

    let added = user::add_secondary_email(&ctx, "alice", "alice@example.com", Locale::En).await;

    let (alice, _) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let alice = alice.unwrap();
    assert!(
        added.is_err() || !alice.secondary_emails.contains("alice@example.com"),
        "the address is listed as both primary and secondary: {:?}",
        alice.secondary_emails
    );
}

/// The other direction, and the one that crosses accounts: a primary claim
/// goes through the `#[entity(index)]` derive, which knows nothing about
/// the secondary index — so nothing stops one account taking an address
/// another account already answers to.
#[tokio::test]
async fn a_primary_claim_cannot_take_an_address_another_account_holds_as_secondary() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "bob", Some("bob@example.com")).await;
    account(&ctx, "alice", None).await;
    user::add_secondary_email(&ctx, "bob", "shared@example.com", Locale::En)
        .await
        .unwrap();

    let taken = user::set_email(&ctx, "alice", Some("shared@example.com"), Locale::En).await;

    assert!(
        taken.is_err(),
        "alice took an address bob already answers to; resolve_by_any_email now \
         attributes bob's commits to alice, since it tries the primary index first"
    );
}

/// A secondary that lost its index entry (see
/// [`own_primary_address_is_refused_as_a_secondary`]) still shows on the
/// account, so removing it must actually free it — otherwise the account
/// keeps an address nobody can resolve and nobody else can claim.
#[tokio::test]
async fn a_secondary_removal_leaves_the_address_free_for_another_account() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;
    account(&ctx, "bob", None).await;
    user::add_secondary_email(&ctx, "alice", "shared@example.com", Locale::En)
        .await
        .unwrap();
    user::remove_secondary_email(&ctx, "alice", "shared@example.com", Locale::En)
        .await
        .unwrap();

    user::add_secondary_email(&ctx, "bob", "shared@example.com", Locale::En)
        .await
        .expect("bob should be able to claim an address alice released");
    let resolved = access::resolve_by_any_email(ctx.backend().as_ref(), &ctx.cipher, "shared@example.com")
        .await
        .unwrap()
        .map(|u| u.username);
    assert_eq!(resolved.as_deref(), Some("bob"));
}
