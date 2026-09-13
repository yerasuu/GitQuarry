use gitquarry_core::git::http::*;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::sync::Arc;

#[test]
fn classify_strips_known_suffixes() {
    assert_eq!(
        classify("/team/proj/info/refs"),
        Some(("team/proj", Kind::InfoRefs, None))
    );
    assert_eq!(
        classify("/team/proj/git-upload-pack"),
        Some(("team/proj", Kind::UploadPack, None))
    );
    assert_eq!(
        classify("/team/proj/git-receive-pack"),
        Some(("team/proj", Kind::ReceivePack, None))
    );
    assert_eq!(classify("/team/proj"), None);
    assert_eq!(classify("/info/refs"), None); // no repo name at all
}

#[test]
fn classify_recognizes_lfs_batch_and_object_endpoints() {
    assert_eq!(
        classify("/team/proj/info/lfs/objects/batch"),
        Some(("team/proj", Kind::LfsBatch, None))
    );
    let oid = "a".repeat(64);
    assert_eq!(
        classify(&format!("/team/proj/info/lfs/objects/{oid}")),
        Some(("team/proj", Kind::LfsObject, Some(oid.as_str())))
    );
    // Trailing segment with a `/` in it isn't a single oid.
    assert_eq!(classify("/team/proj/info/lfs/objects/a/b"), None);
    assert_eq!(classify("/team/proj/info/lfs/objects/"), None);
}

#[test]
fn classify_recognizes_lfs_locking_endpoints() {
    assert_eq!(
        classify("/team/proj/info/lfs/locks"),
        Some(("team/proj", Kind::LfsLocks, None))
    );
    assert_eq!(
        classify("/team/proj/info/lfs/locks/verify"),
        Some(("team/proj", Kind::LfsLocksVerify, None))
    );
    assert_eq!(
        classify("/team/proj/info/lfs/locks/42/unlock"),
        Some(("team/proj", Kind::LfsLockUnlock, Some("42")))
    );
    // A further, unexpected path segment isn't a bare lock id.
    assert_eq!(classify("/team/proj/info/lfs/locks/42/43/unlock"), None);
}

#[test]
fn valid_method_gates_by_kind() {
    assert!(Kind::InfoRefs.valid_method(&Method::GET));
    assert!(!Kind::InfoRefs.valid_method(&Method::POST));
    assert!(Kind::UploadPack.valid_method(&Method::POST));
    assert!(!Kind::UploadPack.valid_method(&Method::GET));
    assert!(Kind::LfsBatch.valid_method(&Method::POST));
    assert!(!Kind::LfsBatch.valid_method(&Method::GET));
    assert!(Kind::LfsObject.valid_method(&Method::GET));
    assert!(Kind::LfsObject.valid_method(&Method::PUT));
    assert!(!Kind::LfsObject.valid_method(&Method::POST));
    assert!(Kind::LfsLocks.valid_method(&Method::GET));
    assert!(Kind::LfsLocks.valid_method(&Method::POST));
    assert!(!Kind::LfsLocks.valid_method(&Method::PUT));
    assert!(Kind::LfsLocksVerify.valid_method(&Method::POST));
    assert!(!Kind::LfsLocksVerify.valid_method(&Method::GET));
    assert!(Kind::LfsLockUnlock.valid_method(&Method::POST));
    assert!(!Kind::LfsLockUnlock.valid_method(&Method::GET));
}

#[test]
fn query_param_finds_the_named_key() {
    assert_eq!(
        query_param("service=git-upload-pack", "service"),
        Some("git-upload-pack")
    );
    assert_eq!(
        query_param("a=1&service=git-receive-pack&b=2", "service"),
        Some("git-receive-pack")
    );
    assert_eq!(query_param("a=1", "service"), None);
    assert_eq!(query_param("", "service"), None);
}

#[test]
fn pkt_line_frames_length_as_four_hex_digits() {
    // "0006" itself is 4 bytes; pkt-line length includes its own header.
    assert_eq!(pkt_line(b""), b"0004");
    assert_eq!(pkt_line(b"ab"), b"0006ab");
    let line = pkt_line(b"# service=git-upload-pack\n");
    assert_eq!(&line[..4], b"001e");
    assert_eq!(&line[4..], b"# service=git-upload-pack\n");
}

#[test]
fn basic_credentials_decodes_user_and_pass() {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, "Basic YWxpY2U6aHVudGVyMg==".parse().unwrap());
    let (user, pass) = basic_credentials(&headers).unwrap();
    assert_eq!(user, "alice");
    assert_eq!(pass, "hunter2");
}

#[test]
fn basic_credentials_none_without_the_header() {
    assert!(basic_credentials(&HeaderMap::new()).is_none());
}

#[test]
fn basic_credentials_none_for_a_bearer_token() {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, "Bearer abc123".parse().unwrap());
    assert!(basic_credentials(&headers).is_none());
}

fn test_ctx(
    root: &std::path::Path,
    federation: Option<gitquarry_core::app::FederationLink>,
) -> gitquarry_core::app::AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Files,
        }),
    );
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[9u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
    );
    gitquarry_core::app::AppContext {
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
        federation,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

fn bearer_username_header(username: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let encoded = BASE64.encode(format!("{username}:"));
    headers.insert(header::AUTHORIZATION, format!("Basic {encoded}").parse().unwrap());
    headers
}

/// Stands in for `gitquarry-federation`'s own
/// `POST /_internal-out/tokens/resolve` (same pattern
/// `webui::federation_proxy::tests::spawn_fake_federation` already
/// uses) — `known_secret` resolves to `("alice", write)`, anything else
/// (or a `repo_uid` other than `"repo-uid-x"`) reports unresolved.
async fn spawn_fake_token_resolver(known_secret: &'static str, write: bool) -> String {
    #[derive(serde::Deserialize)]
    struct Req {
        secret: String,
        repo_uid: String,
    }
    async fn handler(
        axum::extract::State((known_secret, write)): axum::extract::State<(&'static str, bool)>,
        body: Bytes,
    ) -> Response {
        let req: Req = serde_json::from_slice(&body).unwrap();
        let ok = req.secret == known_secret && req.repo_uid == "repo-uid-x";
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::to_vec(&serde_json::json!({
                "ok": ok,
                "username": ok.then_some("alice"),
                "write": ok.then_some(write),
            }))
            .unwrap(),
        )
            .into_response()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/_internal-out/tokens/resolve", axum::routing::post(handler))
        .with_state((known_secret, write));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn authorize_grants_a_token_scoped_repo_at_its_ceiling_and_denies_an_unknown_secret() {
    let base_url = spawn_fake_token_resolver("good-secret", false).await;
    let federation = gitquarry_core::app::FederationLink {
        base_url,
        internal_token: "unused".to_string(),
        http: reqwest::Client::new(),
    };
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), Some(federation));
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
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
    let repo_uid = gitquarry_core::storage::uid::Uid::from("repo-uid-x".to_string());

    // Read-only token (write: false in the fake resolver): read allowed,
    // push denied even though alice owns the repo outright.
    let headers = bearer_username_header("good-secret");
    match authorize(
        &ctx,
        &headers,
        "alice/proj",
        &repo_uid,
        gitquarry_core::storage::entities::Visibility::Private,
        gitquarry_core::auth::entities::Permission::Reporter,
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
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            gitquarry_core::auth::entities::Permission::Developer
        )
        .await
        .unwrap(),
        Authz::Denied(Denial::Forbidden)
    ));

    // Wrong secret: the fake resolver reports unresolved, falls through
    // to the password path, fails there too (no such user "bad-secret").
    let bad_headers = bearer_username_header("bad-secret");
    assert!(matches!(
        authorize(
            &ctx,
            &bad_headers,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            gitquarry_core::auth::entities::Permission::Reporter
        )
        .await
        .unwrap(),
        Authz::Denied(Denial::Unauthorized)
    ));
}

#[tokio::test]
async fn authorize_allows_a_write_token_to_push() {
    let base_url = spawn_fake_token_resolver("rw-secret", true).await;
    let federation = gitquarry_core::app::FederationLink {
        base_url,
        internal_token: "unused".to_string(),
        http: reqwest::Client::new(),
    };
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), Some(federation));
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
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
    let repo_uid = gitquarry_core::storage::uid::Uid::from("repo-uid-x".to_string());
    let headers = bearer_username_header("rw-secret");
    assert!(matches!(
        authorize(
            &ctx,
            &headers,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            gitquarry_core::auth::entities::Permission::Developer
        )
        .await
        .unwrap(),
        Authz::Allowed(Some(_))
    ));
}

#[tokio::test]
async fn authorize_falls_through_to_password_auth_without_federation_configured() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), None);
    let repo_uid = gitquarry_core::storage::uid::Uid::from("repo-uid-x".to_string());
    let headers = bearer_username_header("whatever-secret");
    assert!(matches!(
        authorize(
            &ctx,
            &headers,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            gitquarry_core::auth::entities::Permission::Reporter
        )
        .await
        .unwrap(),
        Authz::Denied(Denial::Unauthorized)
    ));
}

fn basic_auth_header(username: &str, password: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let encoded = BASE64.encode(format!("{username}:{password}"));
    headers.insert(header::AUTHORIZATION, format!("Basic {encoded}").parse().unwrap());
    headers
}

#[tokio::test]
async fn authorize_slows_a_username_down_after_too_many_wrong_passwords_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), None);
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: Some("correct-password"),
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
    ctx.create_repo("alice/proj").await.unwrap().unwrap();
    let repo_uid = gitquarry_core::storage::uid::Uid::from("repo-uid-x".to_string());
    let wrong = basic_auth_header("alice", "wrong-password");

    for _ in 0..gitquarry_core::app::LOGIN_MAX_ATTEMPTS {
        assert!(matches!(
            authorize(
                &ctx,
                &wrong,
                "alice/proj",
                &repo_uid,
                gitquarry_core::storage::entities::Visibility::Private,
                gitquarry_core::auth::entities::Permission::Reporter
            )
            .await
            .unwrap(),
            Authz::Denied(Denial::Unauthorized)
        ));
    }

    // Past the cap the account is *slowed*, never locked: the person who
    // actually knows the password still gets in, so the counter can't be
    // turned into a denial of service against them.
    assert!(!ctx.login_attempts.penalty("alice").is_zero());
    let right = basic_auth_header("alice", "correct-password");
    assert!(matches!(
        authorize(
            &ctx,
            &right,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            gitquarry_core::auth::entities::Permission::Reporter
        )
        .await
        .unwrap(),
        Authz::Allowed(_)
    ));
}

#[tokio::test]
async fn authorize_clears_the_failure_counter_on_a_correct_password() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), None);
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: Some("correct-password"),
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
    ctx.create_repo("alice/proj").await.unwrap().unwrap();
    let repo_uid = gitquarry_core::storage::uid::Uid::from("repo-uid-x".to_string());
    let wrong = basic_auth_header("alice", "wrong-password");
    let right = basic_auth_header("alice", "correct-password");

    for _ in 0..gitquarry_core::app::LOGIN_MAX_ATTEMPTS - 1 {
        authorize(
            &ctx,
            &wrong,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            gitquarry_core::auth::entities::Permission::Reporter,
        )
        .await
        .unwrap();
    }
    // One attempt under the cap -- a correct password still gets through
    // and resets the counter.
    assert!(matches!(
        authorize(
            &ctx,
            &right,
            "alice/proj",
            &repo_uid,
            gitquarry_core::storage::entities::Visibility::Private,
            gitquarry_core::auth::entities::Permission::Reporter
        )
        .await
        .unwrap(),
        Authz::Allowed(Some(_))
    ));
}

/// End-to-end over a real socket (same reason `into_make_service_with_connect_info`
/// matters here at all: [`middleware`] extracts a real `ConnectInfo`, which
/// a `tower::ServiceExt::oneshot` call never provides) — repeats an
/// anonymous clone-refs request against a real `Public` repo past
/// [`gitquarry_core::app::GIT_RATE_LIMIT_ANONYMOUS_MAX`] and confirms the
/// server starts answering `429` instead of `200`.
#[tokio::test]
async fn anonymous_info_refs_gets_rate_limited_past_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), None));
    let resolved = ctx.create_repo("acme/demo").await.unwrap().unwrap();
    let config = gitquarry_core::storage::entities::RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/demo".to_string(),
        visibility: gitquarry_core::storage::entities::Visibility::Public,
        ..Default::default()
    };
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app =
        axum::Router::new()
            .fallback(|| async { StatusCode::NOT_FOUND })
            .layer(axum::middleware::from_fn_with_state(
                ctx,
                gitquarry_core::git::http::middleware,
            ));
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/acme/demo/info/refs?service=git-upload-pack");
    let mut saw_429 = false;
    for _ in 0..gitquarry_core::app::GIT_RATE_LIMIT_ANONYMOUS_MAX + 5 {
        let status = client.get(&url).send().await.unwrap().status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
            break;
        }
        assert_eq!(status, StatusCode::OK);
    }
    assert!(saw_429, "never got rate limited within the cap + margin");
}

#[test]
fn the_rate_limit_key_ignores_a_forwarded_header_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), None);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-forwarded-for", "9.9.9.9".parse().unwrap());
    // Trusting the header without a proxy that owns it would let any client
    // pick its own bucket, i.e. opt out of the limit entirely.
    assert_eq!(
        gitquarry_core::git::http::client_key(&ctx, &headers, "127.0.0.1".parse().unwrap()),
        "127.0.0.1"
    );
}

#[test]
fn the_rate_limit_key_uses_the_first_forwarded_hop_when_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = test_ctx(dir.path(), None);
    ctx.trust_forwarded_for = true;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-forwarded-for", "9.9.9.9, 10.0.0.1".parse().unwrap());
    assert_eq!(
        gitquarry_core::git::http::client_key(&ctx, &headers, "127.0.0.1".parse().unwrap()),
        "9.9.9.9"
    );

    // No header (a request that didn't come through the proxy) falls back
    // to the socket rather than lumping everyone into one empty key.
    assert_eq!(
        gitquarry_core::git::http::client_key(&ctx, &axum::http::HeaderMap::new(), "127.0.0.1".parse().unwrap()),
        "127.0.0.1"
    );
}
