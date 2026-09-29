//! `auth::domain::user::set_email`: the admin-only primary-email
//! operation. `User::email` had no setter at all until this existed — see
//! that function's own doc comment on why an account created without one
//! (every `setup.rs` bootstrap admin) could otherwise never acquire one.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::Permission;
use gitquarry_i18n::Locale;
use gitquarry_test_support::account;

async fn email_of(ctx: &AppContext, username: &str) -> Option<String> {
    access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap()
        .0
        .unwrap()
        .email
}

#[tokio::test]
async fn set_email_gives_an_account_created_without_one_an_address() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;
    assert_eq!(email_of(&ctx, "alice").await, None);

    user::set_email(&ctx, "alice", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();
    assert_eq!(email_of(&ctx, "alice").await.as_deref(), Some("alice@example.com"));
}

/// The address becomes resolvable for OIDC login, which is the whole point
/// of the index claim — setting the field without claiming it would leave
/// the account unreachable by the method the field exists for.
#[tokio::test]
async fn a_newly_set_email_resolves_back_to_its_account() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;
    user::set_email(&ctx, "alice", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();

    let found = access::resolve_by_email(ctx.backend().as_ref(), &ctx.cipher, "alice@example.com")
        .await
        .unwrap();
    assert_eq!(found.map(|u| u.username).as_deref(), Some("alice"));
}

#[tokio::test]
async fn set_email_refuses_an_address_another_account_already_holds() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;
    account(&ctx, "bob", None).await;
    user::set_email(&ctx, "alice", Some("shared@example.com"), Locale::En)
        .await
        .unwrap();

    assert!(
        user::set_email(&ctx, "bob", Some("shared@example.com"), Locale::En)
            .await
            .is_err()
    );
    assert_eq!(email_of(&ctx, "bob").await, None);
    // Alice keeps it, and keeps resolving.
    assert_eq!(email_of(&ctx, "alice").await.as_deref(), Some("shared@example.com"));
}

/// The old claim has to be released, or the account's previous address
/// stays permanently reserved by nobody.
#[tokio::test]
async fn changing_an_email_frees_the_previous_one_for_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;
    account(&ctx, "bob", None).await;
    user::set_email(&ctx, "alice", Some("first@example.com"), Locale::En)
        .await
        .unwrap();
    user::set_email(&ctx, "alice", Some("second@example.com"), Locale::En)
        .await
        .unwrap();

    // Nothing resolves to the old address any more...
    let stale = access::resolve_by_email(ctx.backend().as_ref(), &ctx.cipher, "first@example.com")
        .await
        .unwrap();
    assert!(stale.is_none(), "the old address still resolves");
    // ...and someone else can now take it.
    user::set_email(&ctx, "bob", Some("first@example.com"), Locale::En)
        .await
        .unwrap();
    assert_eq!(email_of(&ctx, "bob").await.as_deref(), Some("first@example.com"));
}

#[tokio::test]
async fn set_email_none_clears_the_address_and_releases_the_claim() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;
    account(&ctx, "bob", None).await;
    user::set_email(&ctx, "alice", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();

    user::set_email(&ctx, "alice", None, Locale::En).await.unwrap();
    assert_eq!(email_of(&ctx, "alice").await, None);
    // Released, not merely detached.
    user::set_email(&ctx, "bob", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();
    assert_eq!(email_of(&ctx, "bob").await.as_deref(), Some("alice@example.com"));
}

#[tokio::test]
async fn set_email_rejects_a_malformed_address_and_an_unknown_user() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;

    assert!(
        user::set_email(&ctx, "alice", Some("no-at-sign"), Locale::En)
            .await
            .is_err()
    );
    assert!(
        user::set_email(&ctx, "nobody", Some("x@example.com"), Locale::En)
            .await
            .is_err()
    );
    assert_eq!(email_of(&ctx, "alice").await, None);
}

/// Setting the address it already has is a no-op, not a self-conflict
/// against its own claim.
#[tokio::test]
async fn set_email_to_the_current_value_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    account(&ctx, "alice", None).await;
    user::set_email(&ctx, "alice", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();

    user::set_email(&ctx, "alice", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();
    assert_eq!(email_of(&ctx, "alice").await.as_deref(), Some("alice@example.com"));
}

async fn private_repo(ctx: &AppContext, full_name: &str) -> gitquarry_core::storage::uid::Uid {
    gitquarry_core::domain::repo::create(
        ctx,
        full_name,
        None,
        None,
        None,
        gitquarry_core::storage::entities::Visibility::Private,
    )
    .await
    .unwrap()
    .unwrap()
    .uid
}

async fn granted_on(ctx: &AppContext, repo_uid: &gitquarry_core::storage::uid::Uid) -> Vec<String> {
    access::list_repo_permissions(&ctx.backend(), &ctx.cipher, repo_uid)
        .await
        .unwrap()
        .into_iter()
        .map(|(u, _)| u.username)
        .collect()
}

/// The repo's own permission grid lists grantees from the repo permission
/// index, so a grant made from the admin user page has to land there too —
/// otherwise it's live but invisible, and can't be revoked from the repo.
#[tokio::test]
async fn a_grant_from_the_user_side_shows_up_on_the_repo_and_its_revocation_too() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let repo_uid = private_repo(&ctx, "team/proj").await;
    account(&ctx, "alice", None).await;

    user::set_repo_perm(&ctx, "alice", "team/proj", Some(Permission::Developer), Locale::En)
        .await
        .unwrap();
    assert_eq!(granted_on(&ctx, &repo_uid).await, ["alice"]);

    user::set_repo_perm(&ctx, "alice", "team/proj", None, Locale::En)
        .await
        .unwrap();
    assert!(granted_on(&ctx, &repo_uid).await.is_empty());
}

#[tokio::test]
async fn repos_granted_at_account_creation_show_up_on_the_repo() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let repo_uid = private_repo(&ctx, "team/proj").await;

    user::create(
        &ctx,
        user::NewUser {
            username: "bob",
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "team/proj:reporter",
            locale: Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(granted_on(&ctx, &repo_uid).await, ["bob"]);
}
