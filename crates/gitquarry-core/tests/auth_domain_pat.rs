use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::{self, user};
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::uid::Uid;
use reqwest::StatusCode;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Creates alice plus her private `alice/proj`, returning her uid and the
/// repo's: a PAT scope names a repo by its uid, not its name.
async fn make_alice(ctx: &AppContext) -> (Uid, Uid) {
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
    let repo = private_repo(ctx, "alice/proj").await;
    let owner = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap()
        .uid;
    (owner, repo)
}

async fn private_repo(ctx: &AppContext, full_name: &str) -> Uid {
    gitquarry_core::domain::repo::create(ctx, full_name, None, None, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap()
        .uid
}

/// Serves `git::http::middleware` on an ephemeral port, the same layer the
/// real server mounts, and returns its base URL.
async fn serve(ctx: Arc<AppContext>) -> String {
    let app = axum::Router::new()
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND })
        .layer(axum::middleware::from_fn_with_state(
            ctx,
            gitquarry_core::git::http::middleware,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("http://{addr}")
}

/// A clone's first request, gated at Reporter. The secret goes in the Basic
/// username field, the `https://<token>@host/...` shape git clients send.
async fn fetch_refs(base: &str, repo: &str, secret: &str) -> StatusCode {
    reqwest::Client::new()
        .get(format!("{base}/{repo}/info/refs?service=git-upload-pack"))
        .basic_auth(secret, Some(""))
        .send()
        .await
        .unwrap()
        .status()
}

/// A push's pack upload, gated at Developer. The empty body is never a valid
/// push, so only a 401/403 from the gate itself says anything here.
async fn push(base: &str, repo: &str, secret: &str) -> StatusCode {
    reqwest::Client::new()
        .post(format!("{base}/{repo}/git-receive-pack"))
        .header("content-type", "application/x-git-receive-pack-request")
        .basic_auth(secret, Some(""))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn create_rejects_empty_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let (owner_uid, _repo) = make_alice(&ctx).await;
    assert!(
        domain::pat::create(&ctx, &owner_uid, "empty", BTreeMap::new(), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn created_secret_resolves_back_to_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let (owner_uid, repo_uid) = make_alice(&ctx).await;
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
async fn a_pat_opens_its_scoped_repo_over_http_and_not_an_unscoped_one() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let (owner_uid, proj) = make_alice(&ctx).await;
    private_repo(&ctx, "alice/other").await;
    let scopes = BTreeMap::from([(proj, Permission::Developer)]);
    let (_token, secret) = domain::pat::create(&ctx, &owner_uid, "laptop", scopes, None)
        .await
        .unwrap()
        .unwrap();
    let base = serve(Arc::clone(&ctx)).await;

    assert_eq!(fetch_refs(&base, "alice/proj", &secret).await, StatusCode::OK);
    let pushed = push(&base, "alice/proj", &secret).await;
    assert!(
        pushed != StatusCode::UNAUTHORIZED && pushed != StatusCode::FORBIDDEN,
        "a Developer-scoped PAT must pass the push gate, got {pushed}"
    );
    // Out of scope the secret isn't a credential for that repo at all, so
    // it's asked to authenticate rather than told it lacks permission.
    assert_eq!(
        fetch_refs(&base, "alice/other", &secret).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_pat_is_capped_at_its_ceiling_never_the_owners_real_permission() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    // alice is implicit Owner of her own namespace, but this token's own
    // ceiling on the repo is only Reporter (read-only).
    let (owner_uid, proj) = make_alice(&ctx).await;
    let scopes = BTreeMap::from([(proj, Permission::Reporter)]);
    let (_token, secret) = domain::pat::create(&ctx, &owner_uid, "readonly", scopes, None)
        .await
        .unwrap()
        .unwrap();
    let base = serve(Arc::clone(&ctx)).await;

    assert_eq!(fetch_refs(&base, "alice/proj", &secret).await, StatusCode::OK);
    assert_eq!(push(&base, "alice/proj", &secret).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_expired_pat_is_refused_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let (owner_uid, proj) = make_alice(&ctx).await;
    let scopes = BTreeMap::from([(proj, Permission::Developer)]);
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
    let base = serve(Arc::clone(&ctx)).await;

    assert_eq!(fetch_refs(&base, "alice/proj", &secret).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoke_removes_the_token_and_its_owner_index_entry() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let (owner_uid, repo_uid) = make_alice(&ctx).await;
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
    let ctx = gitquarry_test_support::ctx(dir.path());
    let (owner_uid, repo_uid) = make_alice(&ctx).await;
    let other_uid = Uid::new();
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
