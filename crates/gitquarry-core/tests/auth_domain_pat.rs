use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::{self, user};
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::git::http::{Authz, Denial, authorize};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::uid::Uid;
use std::collections::BTreeMap;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        tracker: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn make_alice(ctx: &AppContext) -> Uid {
    user::create(
        ctx,
        user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
    ctx.create_repo("alice/proj").await.unwrap().unwrap();
    access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap()
        .uid
}

fn basic_auth_header(username_field: &str) -> axum::http::HeaderMap {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    let mut headers = axum::http::HeaderMap::new();
    let encoded = BASE64.encode(format!("{username_field}:"));
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Basic {encoded}").parse().unwrap(),
    );
    headers
}

#[tokio::test]
async fn create_rejects_empty_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner_uid = make_alice(&ctx).await;
    assert!(
        domain::pat::create(&ctx, &owner_uid, "empty", BTreeMap::new(), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn created_secret_resolves_back_to_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner_uid = make_alice(&ctx).await;
    let repo_uid = Uid::from("repo-uid-x".to_string());
    let scopes = BTreeMap::from([(repo_uid, Permission::Developer)]);
    let (token, secret) = domain::pat::create(&ctx, &owner_uid, "laptop", scopes, None)
        .await
        .unwrap()
        .unwrap();

    let resolved = access::resolve_by_secret(ctx.backend().as_ref(), &ctx.cipher, &secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.uid, token.uid);
    assert!(secret.starts_with("gqpat_"));
}

#[tokio::test]
async fn authorize_allows_the_scoped_repo_and_denies_an_unscoped_one() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner_uid = make_alice(&ctx).await;
    let repo_uid_a = Uid::from("repo-a".to_string());
    let repo_uid_b = Uid::from("repo-b".to_string());
    let scopes = BTreeMap::from([(repo_uid_a.clone(), Permission::Developer)]);
    let (_token, secret) = domain::pat::create(&ctx, &owner_uid, "laptop", scopes, None)
        .await
        .unwrap()
        .unwrap();
    let headers = basic_auth_header(&secret);

    match authorize(
        &ctx,
        &headers,
        "alice/proj",
        &repo_uid_a,
        gitquarry_core::storage::entities::Visibility::Private,
        Permission::Developer,
    )
    .await
    .unwrap()
    {
        Authz::Allowed(Some(username)) => assert_eq!(username, "alice"),
        other => panic!("expected Allowed, got {other:?}"),
    }

    assert!(matches!(
        authorize(
            &ctx,
            &headers,
            "alice/proj",
            &repo_uid_b,
            gitquarry_core::storage::entities::Visibility::Private,
            Permission::Reporter,
        )
        .await
        .unwrap(),
        Authz::Denied(Denial::Unauthorized)
    ));
}

#[tokio::test]
async fn authorize_caps_at_the_tokens_ceiling_never_the_owners_real_permission() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner_uid = make_alice(&ctx).await;
    let repo_uid = Uid::from("repo-a".to_string());
    // alice is implicit Owner of her own namespace, but this token's own
    // ceiling on the repo is only Reporter (read-only).
    let scopes = BTreeMap::from([(repo_uid.clone(), Permission::Reporter)]);
    let (_token, secret) = domain::pat::create(&ctx, &owner_uid, "readonly", scopes, None)
        .await
        .unwrap()
        .unwrap();
    let headers = basic_auth_header(&secret);

    assert!(matches!(
        authorize(
            &ctx,
            &headers,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            Permission::Reporter,
        )
        .await
        .unwrap(),
        Authz::Allowed(Some(_))
    ));
    assert!(matches!(
        authorize(
            &ctx,
            &headers,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            Permission::Developer,
        )
        .await
        .unwrap(),
        Authz::Denied(Denial::Forbidden)
    ));
}

#[tokio::test]
async fn authorize_denies_an_expired_token() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner_uid = make_alice(&ctx).await;
    let repo_uid = Uid::from("repo-a".to_string());
    let scopes = BTreeMap::from([(repo_uid.clone(), Permission::Developer)]);
    let (token, secret) = domain::pat::create(&ctx, &owner_uid, "old", scopes, Some(1))
        .await
        .unwrap()
        .unwrap();

    // Force it into the past directly (real expiry is relative to
    // creation time, too slow to wait out in a test).
    let (stale, etag) = gitquarry_core::auth::entities::PersonalAccessToken::get(
        ctx.backend().as_ref(),
        &ctx.cipher,
        token.uid.as_str(),
    )
    .await
    .unwrap();
    let mut stale = stale.unwrap();
    stale.expires_at = Some(gitquarry_core::storage::audit::format_unix_secs(0));
    stale
        .put(ctx.backend().as_ref(), &ctx.cipher, etag.as_deref())
        .await
        .unwrap();

    let headers = basic_auth_header(&secret);
    assert!(matches!(
        authorize(
            &ctx,
            &headers,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            Permission::Reporter,
        )
        .await
        .unwrap(),
        Authz::Denied(Denial::Unauthorized)
    ));
}

#[tokio::test]
async fn revoke_removes_the_token_and_its_owner_index_entry() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner_uid = make_alice(&ctx).await;
    let repo_uid = Uid::from("repo-a".to_string());
    let scopes = BTreeMap::from([(repo_uid, Permission::Developer)]);
    let (token, secret) = domain::pat::create(&ctx, &owner_uid, "laptop", scopes, None)
        .await
        .unwrap()
        .unwrap();

    domain::pat::revoke(&ctx, &owner_uid, token.uid.as_str(), false)
        .await
        .unwrap();

    assert!(
        access::resolve_by_secret(ctx.backend().as_ref(), &ctx.cipher, &secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        access::list_tokens_for_owner(ctx.backend().as_ref(), &ctx.cipher, &owner_uid)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn revoke_is_a_no_op_for_a_non_owner_without_admin() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let owner_uid = make_alice(&ctx).await;
    let other_uid = Uid::new();
    let repo_uid = Uid::from("repo-a".to_string());
    let scopes = BTreeMap::from([(repo_uid, Permission::Developer)]);
    let (token, secret) = domain::pat::create(&ctx, &owner_uid, "laptop", scopes, None)
        .await
        .unwrap()
        .unwrap();

    domain::pat::revoke(&ctx, &other_uid, token.uid.as_str(), false)
        .await
        .unwrap();

    assert!(
        access::resolve_by_secret(ctx.backend().as_ref(), &ctx.cipher, &secret)
            .await
            .unwrap()
            .is_some()
    );
}
