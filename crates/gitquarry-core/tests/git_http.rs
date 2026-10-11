//! Drives `git::http::middleware` over a real socket, because it extracts
//! `ConnectInfo` (the anonymous rate limit keys on the peer address), which
//! only `into_make_service_with_connect_info` provides.

use gitquarry_core::app::{AppContext, FederationLink};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::PASSWORD;

use axum::body::Bytes;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Mirrors `app::LOGIN_MAX_ATTEMPTS`.
const LOGIN_MAX_ATTEMPTS: usize = 5;
/// Mirrors `app::GIT_RATE_LIMIT_ANONYMOUS_MAX`.
const GIT_RATE_LIMIT_ANONYMOUS_MAX: usize = 30;

/// Whatever the middleware passes through lands here, so a test can tell
/// "not a git/LFS endpoint" apart from any status the git handler returns.
const PASSTHROUGH: StatusCode = StatusCode::IM_A_TEAPOT;

/// Kept beside the shared `user` because some tests need a password with a
/// colon in it, or none at all.
async fn create_user(ctx: &AppContext, username: &str, password: Option<&str>) {
    gitquarry_core::auth::domain::user::create(
        ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: password,
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
}

async fn create_repo(ctx: &AppContext, full_name: &str, visibility: Visibility) -> Uid {
    let resolved = ctx.create_repo(full_name).await.unwrap().unwrap();
    let backend = ctx.backend();
    let (mut config, etag) =
        gitquarry_core::storage::settings::get_repo_config(backend.as_ref(), resolved.uid.as_str(), &ctx.cipher)
            .await
            .unwrap();
    config.visibility = visibility;
    gitquarry_core::storage::settings::put_repo_config(
        backend.as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();
    resolved.uid
}

/// Returns the base URL of a server that runs only the git middleware over
/// a [`PASSTHROUGH`] fallback.
async fn serve(ctx: Arc<AppContext>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .fallback(|| async { PASSTHROUGH })
        .layer(axum::middleware::from_fn_with_state(
            ctx,
            gitquarry_core::git::http::middleware,
        ));
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

/// A client whose connections originate from `ip` (any `127.x.y.z` works on
/// Linux loopback), so tests can play distinct anonymous peers.
fn client_from(ip: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .local_address(ip.parse::<IpAddr>().unwrap())
        .build()
        .unwrap()
}

fn upload_pack_refs(client: &reqwest::Client, base: &str, repo: &str) -> reqwest::RequestBuilder {
    client.get(format!("{base}/{repo}/info/refs?service=git-upload-pack"))
}

#[tokio::test]
async fn git_and_lfs_endpoints_are_intercepted_with_the_method_each_expects() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    create_repo(&ctx, "team/proj", Visibility::Private).await;
    let base = serve(ctx).await;
    let client = reqwest::Client::new();
    let oid = "a".repeat(64);

    // Anonymous against a private repo: an intercepted endpoint that
    // resolved `team/proj` answers 401, a wrong method falls through.
    for (path, right, wrong) in [
        (
            "info/refs?service=git-upload-pack".to_string(),
            vec![Method::GET],
            vec![Method::POST],
        ),
        ("git-upload-pack".to_string(), vec![Method::POST], vec![Method::GET]),
        ("git-receive-pack".to_string(), vec![Method::POST], vec![Method::GET]),
        (
            "info/lfs/objects/batch".to_string(),
            vec![Method::POST],
            vec![Method::GET],
        ),
        (
            format!("info/lfs/objects/{oid}"),
            vec![Method::GET, Method::PUT],
            vec![Method::POST],
        ),
        (
            "info/lfs/locks".to_string(),
            vec![Method::GET, Method::POST],
            vec![Method::PUT],
        ),
        (
            "info/lfs/locks/verify".to_string(),
            vec![Method::POST],
            vec![Method::GET],
        ),
        (
            "info/lfs/locks/42/unlock".to_string(),
            vec![Method::POST],
            vec![Method::GET],
        ),
    ] {
        let url = format!("{base}/team/proj/{path}");
        for method in right {
            let status = client.request(method.clone(), &url).send().await.unwrap().status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        }
        for method in wrong {
            let status = client.request(method.clone(), &url).send().await.unwrap().status();
            assert_eq!(status, PASSTHROUGH, "{method} {path}");
        }
    }

    // A recognized suffix whose prefix names no repo is the browsing UI's
    // to answer: a file called `info/refs` or `info/lfs/locks` viewed
    // under `/team/proj/blob/main/` looks exactly like a git endpoint.
    let status = upload_pack_refs(&client, &base, "nobody/here")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, PASSTHROUGH);

    for path in [
        "/team/proj/blob/main/info/refs",
        "/team/proj/blob/main/info/lfs/locks",
        "/team/proj",
        "/info/refs",
        "/team/proj/info/lfs/objects/a/b",
        "/team/proj/info/lfs/objects/",
        "/team/proj/info/lfs/locks/42/43/unlock",
    ] {
        let status = client.get(format!("{base}{path}")).send().await.unwrap().status();
        let post_status = client.post(format!("{base}{path}")).send().await.unwrap().status();
        assert_eq!(status, PASSTHROUGH, "GET {path}");
        assert_eq!(post_status, PASSTHROUGH, "POST {path}");
    }
}

#[tokio::test]
async fn info_refs_advertises_the_service_named_anywhere_in_the_query() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    create_repo(&ctx, "acme/demo", Visibility::Public).await;
    let base = serve(ctx).await;
    let client = reqwest::Client::new();

    // The preamble is a pkt-line whose 4-hex-digit length counts its own
    // header: 26 + 4 = 0x1e, 27 + 4 = 0x1f; then a flush-pkt.
    for (query, service, preamble) in [
        (
            "service=git-upload-pack",
            "git-upload-pack",
            &b"001e# service=git-upload-pack\n0000"[..],
        ),
        (
            "a=1&service=git-receive-pack&b=2",
            "git-receive-pack",
            &b"001f# service=git-receive-pack\n0000"[..],
        ),
    ] {
        let resp = client
            .get(format!("{base}/acme/demo/info/refs?{query}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{query}");
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            format!("application/x-{service}-advertisement").as_str()
        );
        let body = resp.bytes().await.unwrap();
        assert!(body.starts_with(preamble), "{query}: {body:?}");
    }

    for url in [
        format!("{base}/acme/demo/info/refs?a=1"),
        format!("{base}/acme/demo/info/refs"),
        format!("{base}/acme/demo/info/refs?service=git-evil"),
    ] {
        let status = client.get(&url).send().await.unwrap().status();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}");
    }
}

#[tokio::test]
async fn a_private_repo_asks_for_basic_credentials_when_none_are_usable() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    create_repo(&ctx, "acme/demo", Visibility::Private).await;
    let base = serve(ctx).await;
    let client = reqwest::Client::new();

    // A Bearer token isn't Basic auth, so it counts as no credentials at all.
    for req in [
        upload_pack_refs(&client, &base, "acme/demo"),
        upload_pack_refs(&client, &base, "acme/demo").bearer_auth("abc123"),
        upload_pack_refs(&client, &base, "acme/demo").header(header::AUTHORIZATION, "Basic !!not-base64!!"),
    ] {
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(resp.headers()[header::WWW_AUTHENTICATE], "Basic realm=\"gitquarry\"");
        assert_eq!(resp.text().await.unwrap(), "credentials required");
    }
}

#[tokio::test]
async fn basic_credentials_split_at_the_first_colon_so_a_password_may_contain_one() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    create_user(&ctx, "alice", Some("hunter:2")).await;
    create_repo(&ctx, "alice/proj", Visibility::Private).await;
    let base = serve(ctx).await;
    let client = reqwest::Client::new();

    let right = upload_pack_refs(&client, &base, "alice/proj")
        .basic_auth("alice", Some("hunter:2"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(right, StatusCode::OK);
    let wrong = upload_pack_refs(&client, &base, "alice/proj")
        .basic_auth("alice", Some("hunter"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(wrong, StatusCode::UNAUTHORIZED);
}

/// Stands in for `gitquarry-federation`'s own
/// `POST /_internal-out/tokens/resolve`: `known_secret` resolves to
/// `("alice", write)` for `repo_uid` only, anything else reports unresolved.
async fn spawn_fake_token_resolver(known_secret: &'static str, write: bool, repo_uid: Uid) -> String {
    #[derive(serde::Deserialize)]
    struct Req {
        secret: String,
        repo_uid: String,
    }
    async fn handler(
        axum::extract::State((known_secret, write, repo_uid)): axum::extract::State<(&'static str, bool, Uid)>,
        body: Bytes,
    ) -> Response {
        let req: Req = serde_json::from_slice(&body).unwrap();
        let ok = req.secret == known_secret && req.repo_uid == repo_uid.as_str();
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
        .with_state((known_secret, write, repo_uid));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// A server where alice owns the private `alice/proj` and federation
/// resolves `secret` as a token on it.
async fn serve_with_federation_token(dir: &std::path::Path, secret: &'static str, write: bool) -> String {
    // The resolver needs the repo's uid, which only exists once the repo
    // does, so the context gets its federation link after the fact.
    let mut ctx = gitquarry_test_support::ctx(dir);
    create_user(&ctx, "alice", None).await;
    let repo_uid = create_repo(&ctx, "alice/proj", Visibility::Private).await;
    ctx.federation = Some(FederationLink {
        base_url: spawn_fake_token_resolver(secret, write, repo_uid).await,
        internal_token: "unused".to_string(),
        http: reqwest::Client::new(),
    });
    serve(Arc::new(ctx)).await
}

/// An empty receive-pack request (just a flush-pkt): no ref updates, so it
/// succeeds for anyone allowed to push without changing the repo.
fn empty_push(client: &reqwest::Client, base: &str, secret: &str) -> reqwest::RequestBuilder {
    client
        .post(format!("{base}/alice/proj/git-receive-pack"))
        .basic_auth(secret, None::<&str>)
        .body("0000")
}

#[tokio::test]
async fn a_read_only_federation_token_can_fetch_but_not_push_and_an_unknown_one_is_unauthorized() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_with_federation_token(dir.path(), "good-secret", false).await;
    let client = reqwest::Client::new();

    let fetch = upload_pack_refs(&client, &base, "alice/proj")
        .basic_auth("good-secret", None::<&str>)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(fetch, StatusCode::OK);

    // Denied even though alice owns the repo outright: `write: false` caps
    // the token at read-only.
    let push = empty_push(&client, &base, "good-secret").send().await.unwrap();
    assert_eq!(push.status(), StatusCode::FORBIDDEN);
    assert_eq!(push.text().await.unwrap(), "no permission");

    // Unresolved, so it falls through to the password path and fails there
    // too (no such user "bad-secret").
    let bad = upload_pack_refs(&client, &base, "alice/proj")
        .basic_auth("bad-secret", None::<&str>)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(bad, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_write_federation_token_can_push() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_with_federation_token(dir.path(), "rw-secret", true).await;
    let status = empty_push(&reqwest::Client::new(), &base, "rw-secret")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_token_is_just_an_unknown_username_without_federation_configured() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    create_user(&ctx, "alice", None).await;
    create_repo(&ctx, "alice/proj", Visibility::Private).await;
    let base = serve(ctx).await;
    let status = upload_pack_refs(&reqwest::Client::new(), &base, "alice/proj")
        .basic_auth("whatever-secret", None::<&str>)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn too_many_wrong_passwords_slow_a_username_down_without_locking_its_owner_out() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    gitquarry_test_support::user(&ctx, "alice", false).await;
    create_repo(&ctx, "alice/proj", Visibility::Private).await;
    let base = serve(Arc::clone(&ctx)).await;
    let client = reqwest::Client::new();

    for _ in 0..LOGIN_MAX_ATTEMPTS {
        let status = upload_pack_refs(&client, &base, "alice/proj")
            .basic_auth("alice", Some("wrong-password"))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert!(!ctx.login_attempts.penalty("alice").is_zero());

    // Past the cap the account is slowed, never locked: the person who
    // actually knows the password still gets in, so the counter can't be
    // turned into a denial of service against them. The first penalty step
    // is one second.
    let started = Instant::now();
    let status = upload_pack_refs(&client, &base, "alice/proj")
        .basic_auth("alice", Some(PASSWORD))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);
    assert!(started.elapsed() >= Duration::from_secs(1));
}

#[tokio::test]
async fn a_correct_password_clears_the_failure_counter() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    gitquarry_test_support::user(&ctx, "alice", false).await;
    create_repo(&ctx, "alice/proj", Visibility::Private).await;
    let base = serve(Arc::clone(&ctx)).await;
    let client = reqwest::Client::new();
    let attempt = |password: &'static str| {
        upload_pack_refs(&client, &base, "alice/proj")
            .basic_auth("alice", Some(password))
            .send()
    };

    for _ in 0..LOGIN_MAX_ATTEMPTS - 1 {
        assert_eq!(
            attempt("wrong-password").await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(attempt(PASSWORD).await.unwrap().status(), StatusCode::OK);
    // Had the counter survived, this would be failure number
    // LOGIN_MAX_ATTEMPTS and start the penalty.
    assert_eq!(
        attempt("wrong-password").await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(ctx.login_attempts.penalty("alice").is_zero());
}

/// Sends `GIT_RATE_LIMIT_ANONYMOUS_MAX` anonymous fetches that must all be
/// served, built by `request` from the attempt's index.
async fn spend_anonymous_budget(request: impl Fn(usize) -> reqwest::RequestBuilder) {
    for i in 0..GIT_RATE_LIMIT_ANONYMOUS_MAX {
        assert_eq!(request(i).send().await.unwrap().status(), StatusCode::OK, "attempt {i}");
    }
}

async fn serve_public_repo(dir: &std::path::Path, trust_forwarded_for: bool) -> String {
    let ctx = AppContext {
        trust_forwarded_for,
        ..gitquarry_test_support::ctx(dir)
    };
    create_repo(&ctx, "acme/demo", Visibility::Public).await;
    serve(Arc::new(ctx)).await
}

#[tokio::test]
async fn anonymous_info_refs_gets_rate_limited_past_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_public_repo(dir.path(), false).await;
    let client = reqwest::Client::new();
    spend_anonymous_budget(|_| upload_pack_refs(&client, &base, "acme/demo")).await;
    let resp = upload_pack_refs(&client, &base, "acme/demo").send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_push_advertisement_is_not_counted_against_the_clone_rate_limit() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_public_repo(dir.path(), false).await;
    let client = reqwest::Client::new();
    let push_refs = || client.get(format!("{base}/acme/demo/info/refs?service=git-receive-pack"));
    spend_anonymous_budget(|_| push_refs()).await;
    assert_eq!(push_refs().send().await.unwrap().status(), StatusCode::OK);
    // The fetch budget is still whole.
    spend_anonymous_budget(|_| upload_pack_refs(&client, &base, "acme/demo")).await;
}

#[tokio::test]
async fn the_rate_limit_ignores_a_forwarded_header_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_public_repo(dir.path(), false).await;
    let client = reqwest::Client::new();
    // Trusting the header without a proxy that owns it would let any client
    // pick its own bucket, i.e. opt out of the limit entirely.
    let forwarded_as =
        |i: usize| upload_pack_refs(&client, &base, "acme/demo").header("x-forwarded-for", format!("9.9.9.{i}"));
    spend_anonymous_budget(forwarded_as).await;
    let status = forwarded_as(GIT_RATE_LIMIT_ANONYMOUS_MAX)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_trusted_forwarded_header_keys_the_rate_limit_on_its_last_hop() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_public_repo(dir.path(), true).await;
    let client = reqwest::Client::new();
    let forwarded = |value: String| upload_pack_refs(&client, &base, "acme/demo").header("x-forwarded-for", value);

    // The proxy appends the address it saw; whatever the client prepends
    // varies freely and must not buy it a fresh bucket.
    spend_anonymous_budget(|i| forwarded(format!("10.0.0.{i}, 9.9.9.9"))).await;
    let exhausted = forwarded("10.0.0.250, 9.9.9.9".to_string())
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(exhausted, StatusCode::TOO_MANY_REQUESTS);
    let other_client = forwarded("9.9.9.9, 8.8.8.8".to_string()).send().await.unwrap().status();
    assert_eq!(other_client, StatusCode::OK);
}

#[tokio::test]
async fn without_the_forwarded_header_a_trusting_server_keys_the_rate_limit_on_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve_public_repo(dir.path(), true).await;
    // A request that didn't come through the proxy falls back to its own
    // peer address rather than lumping everyone into one empty key.
    let first_peer = client_from("127.0.0.2");
    spend_anonymous_budget(|_| upload_pack_refs(&first_peer, &base, "acme/demo")).await;
    let exhausted = upload_pack_refs(&first_peer, &base, "acme/demo")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(exhausted, StatusCode::TOO_MANY_REQUESTS);
    let second_peer = client_from("127.0.0.3");
    let status = upload_pack_refs(&second_peer, &base, "acme/demo")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);
}
