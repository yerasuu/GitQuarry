//! `auth::domain::user::set_email`: the admin-only primary-email
//! operation. `User::email` had no setter at all until this existed — see
//! that function's own doc comment on why an account created without one
//! (every `setup.rs` bootstrap admin) could otherwise never acquire one.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::user;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_i18n::Locale;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[11u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn make_user(ctx: &AppContext, username: &str) {
    user::create(
        ctx,
        user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

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
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
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
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
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
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    make_user(&ctx, "bob").await;
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
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    make_user(&ctx, "bob").await;
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
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    make_user(&ctx, "bob").await;
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
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;

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
    let ctx = test_ctx(dir.path());
    make_user(&ctx, "alice").await;
    user::set_email(&ctx, "alice", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();

    user::set_email(&ctx, "alice", Some("alice@example.com"), Locale::En)
        .await
        .unwrap();
    assert_eq!(email_of(&ctx, "alice").await.as_deref(), Some("alice@example.com"));
}
