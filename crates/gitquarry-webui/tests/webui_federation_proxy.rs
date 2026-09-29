//! The federation proxy, driven through `router()` against a stand-in for
//! `gitquarry-federation` that records every request it receives.
//!
//! Federation trusts whatever core sends it — `X-Internal-Token` is its only
//! gate, and `X-Gitquarry-User`/`-User-Uid`/`-Csrf` are taken at face value
//! — so what matters here is less what reaches it than what doesn't: a
//! client's own copies of those headers, its cookies, a path that climbs
//! out of `/_api/*`, or a mutation whose CSRF token core never checked.

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gitquarry_core::app::{AppContext, FederationLink};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery";
const INTERNAL_TOKEN: &str = "core-to-federation-secret";
const CLIENT_IP: [u8; 4] = [203, 0, 113, 7];

struct Received {
    method: String,
    uri: String,
    headers: HeaderMap,
    body: Bytes,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

async fn record(State(tx): State<UnboundedSender<Received>>, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let _ = tx.send(Received {
        method: parts.method.to_string(),
        uri: parts.uri.to_string(),
        headers: parts.headers,
        body,
    });
    (
        StatusCode::CREATED,
        [("x-federation-marker", "relayed"), ("keep-alive", "timeout=5")],
        "from federation",
    )
        .into_response()
}

/// A federation listener on a real socket, and the requests it receives.
async fn spawn_fake_federation() -> (String, UnboundedReceiver<Received>) {
    let (tx, rx) = unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(record).with_state(tx);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), rx)
}

fn test_ctx(root: &std::path::Path, federation_url: Option<String>) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[31u8; s3db_crypto::SALT_LEN]).unwrap());
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
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: federation_url.map(|base_url| FederationLink {
            base_url,
            internal_token: INTERNAL_TOKEN.to_string(),
            http: reqwest::Client::new(),
        }),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// A core wired to a fresh fake federation.
async fn wired(root: &std::path::Path) -> (Arc<AppContext>, axum::Router, UnboundedReceiver<Received>) {
    let (url, rx) = spawn_fake_federation().await;
    let ctx = Arc::new(test_ctx(root, Some(url)));
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    (ctx, app, rx)
}

async fn add_user(ctx: &AppContext, username: &str, is_admin: bool) {
    gitquarry_core::auth::domain::user::create(
        ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
    // `run_server` serves with `into_make_service_with_connect_info`, which
    // is where the proxy's `X-Forwarded-For` comes from.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((CLIENT_IP, 4242))));
    app.clone().oneshot(request).await.unwrap()
}

async fn get(app: &axum::Router, path: &str, cookie: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::empty()).unwrap()).await
}

async fn post_form(app: &axum::Router, path: &str, cookie: Option<&str>, body: &str) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::from(body.to_string())).unwrap()).await
}

async fn body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn csrf_in(html: &str) -> Option<String> {
    let rest = &html[html.find(r#"name="csrf""#)?..];
    let rest = &rest[rest.find(r#"value=""#)? + r#"value=""#.len()..];
    Some(rest[..rest.find('"')?].to_string())
}

fn set_cookie_pair(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap_or_default().to_string())
}

/// `(web-session=... pair, the CSRF token pages render for it)` from a real
/// password login.
async fn login(app: &axum::Router, username: &str) -> (String, String) {
    let response = get(app, "/_login", None).await;
    let anchor = set_cookie_pair(&response, "login-csrf").expect("no login-csrf cookie");
    let token = csrf_in(&body_text(response).await).expect("no csrf field on /_login");
    let form = form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("username", username), ("password", PASSWORD), ("csrf", &token)])
        .finish();
    let response = post_form(app, "/_login", Some(&anchor), &form).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let session = set_cookie_pair(&response, "web-session").expect("login set no session cookie");
    let html = body_text(get(app, "/_account/security", Some(&session)).await).await;
    let csrf = csrf_in(&html).expect("no csrf field on an authenticated page");
    (session, csrf)
}

/// The one request federation received for the call just made.
fn received(rx: &mut UnboundedReceiver<Received>) -> Received {
    let got = rx.try_recv().expect("federation received nothing");
    assert!(rx.try_recv().is_err(), "federation received more than one request");
    got
}

fn assert_nothing_forwarded(rx: &mut UnboundedReceiver<Received>, what: &str) {
    assert!(rx.try_recv().is_err(), "{what} reached federation");
}

fn assert_signed(got: &Received) {
    assert_eq!(got.header("x-internal-token"), Some(INTERNAL_TOKEN));
    assert_eq!(got.headers.get_all("x-internal-token").iter().count(), 1);
    assert_eq!(
        got.header(gitquarry_wire::WIRE_VERSION_HEADER),
        Some(gitquarry_wire::WIRE_PROTOCOL_VERSION.to_string().as_str())
    );
}

#[tokio::test]
async fn a_public_api_call_forwards_its_bearer_token_and_nothing_session_related() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, mut rx) = wired(dir.path()).await;
    let request = Request::builder()
        .uri("/_api/repos/team/project?ref=main")
        .header(header::AUTHORIZATION, "Bearer token_abc")
        .header(header::COOKIE, "web-session=sealed-abc")
        // A client asserting an identity federation would trust verbatim.
        .header("x-gitquarry-user", "mallory")
        .header("x-gitquarry-user-uid", "forged-uid")
        .header("x-gitquarry-csrf", "forged-csrf")
        .header("x-internal-token", "forged-token")
        .header("x-forwarded-for", "198.51.100.1")
        .header("x-client-extra", "1")
        .body(Body::empty())
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let got = received(&mut rx);
    assert_eq!(got.method, "GET");
    assert_eq!(got.uri, "/_api/repos/team/project?ref=main");
    assert_signed(&got);
    assert_eq!(got.header("authorization"), Some("Bearer token_abc"));
    // The address core saw, not whatever the client claimed.
    assert_eq!(got.header("x-forwarded-for"), Some("203.0.113.7"));
    assert_eq!(got.headers.get_all("x-forwarded-for").iter().count(), 1);
    for absent in [
        "cookie",
        "x-gitquarry-user",
        "x-gitquarry-user-uid",
        "x-gitquarry-csrf",
        "x-client-extra",
    ] {
        assert!(
            got.headers.get(absent).is_none(),
            "{absent} was forwarded to federation"
        );
    }
}

/// Regression: a proxied `x-www-form-urlencoded` POST used to arrive at
/// federation with no `Content-Type` at all, and axum's `Form` extractor
/// there 415s without it.
#[tokio::test]
async fn a_public_api_post_forwards_body_and_content_type_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, mut rx) = wired(dir.path()).await;
    let response = post_form(&app, "/_api/pulls", None, "title=a%2Bb&body=x").await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let got = received(&mut rx);
    assert_eq!(got.method, "POST");
    assert_eq!(got.uri, "/_api/pulls");
    assert_eq!(got.header("content-type"), Some("application/x-www-form-urlencoded"));
    assert_eq!(&got.body[..], b"title=a%2Bb&body=x");
    assert!(got.headers.get("authorization").is_none());
}

/// Federation's status, headers and body come back as-is, minus the ones
/// describing its own hop.
#[tokio::test]
async fn federation_s_response_is_relayed_without_its_hop_by_hop_headers() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    // Otherwise the stripping below would pass vacuously: federation's own
    // listener really does send the header.
    let base_url = &ctx.federation.as_ref().unwrap().base_url;
    let direct = reqwest::get(format!("{base_url}/_api/instance")).await.unwrap();
    assert!(direct.headers().get("keep-alive").is_some());
    received(&mut rx);

    let response = get(&app, "/_api/instance", None).await;
    received(&mut rx);
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["x-federation-marker"], "relayed");
    assert!(response.headers().get("keep-alive").is_none());
    assert!(response.headers().get(header::CONNECTION).is_none());
    assert_eq!(body_text(response).await, "from federation");
}

#[tokio::test]
async fn an_unreachable_federation_is_a_bad_gateway() {
    let dir = tempfile::tempdir().unwrap();
    // Port 1: nothing listens there, so the connect is refused outright.
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(dir.path(), Some("http://127.0.0.1:1".to_string()))));
    let response = get(&app, "/_api/instance", None).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn every_federation_route_is_unavailable_without_federation() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), None));
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    for path in [
        "/_api/instance",
        "/_api/repos/team/project",
        "/_federation/gossip/peers",
    ] {
        let response = get(&app, path, None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "GET {path}");
    }
    // `/_peers` is admin-gated first, so it only reports that to an admin.
    add_user(&ctx, "alice", true).await;
    let (session, _csrf) = login(&app, "alice").await;
    let response = get(&app, "/_peers", Some(&session)).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// `/_api/repos/{*name}` forwards a client-controlled path, and `reqwest`
/// resolves dot segments before sending — `../../_internal-out/peers` would
/// otherwise land on a federation route whose only gate is the internal
/// token core attaches.
#[tokio::test]
async fn a_path_that_climbs_out_of_the_api_surface_never_reaches_federation() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, mut rx) = wired(dir.path()).await;
    for path in [
        "/_api/repos/../../_internal-out/peers",
        "/_api/repos/%2e%2e/%2e%2e/_peers",
        "/_api/repos/%2E%2E/_peers",
        "/_api/repos/.%2e/_peers",
        "/_api/repos/./_peers",
    ] {
        let response = get(&app, path, None).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "GET {path}");
        assert_nothing_forwarded(&mut rx, path);
    }
}

#[tokio::test]
async fn ordinary_api_paths_are_forwarded_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, mut rx) = wired(dir.path()).await;
    // A repo name may contain dots, just never a bare `..` segment, and the
    // query half is federation's own business — only the path can traverse.
    for path in ["/_api/repos/team/my.repo.name", "/_api/repos/team/project?path=../etc"] {
        let response = get(&app, path, None).await;
        assert_eq!(response.status(), StatusCode::CREATED, "GET {path}");
        assert_eq!(received(&mut rx).uri, path);
    }
}

#[tokio::test]
async fn peers_is_gated_before_anything_is_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    add_user(&ctx, "bob", false).await;

    let response = get(&app, "/_peers", None).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/_login?return_to=/_peers");
    assert_nothing_forwarded(&mut rx, "an anonymous /_peers");

    let (session, _csrf) = login(&app, "bob").await;
    let response = get(&app, "/_peers", Some(&session)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_nothing_forwarded(&mut rx, "a non-admin's /_peers");
}

/// Before any login method exists no session can, so the only credential
/// that opens an admin page is the bootstrap grant — `/_peers` included.
#[tokio::test]
async fn a_fresh_instance_keeps_peers_closed_without_the_bootstrap_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, mut rx) = wired(dir.path()).await;

    let response = get(&app, "/_peers", None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_nothing_forwarded(&mut rx, "an anonymous /_peers on a fresh instance");
}

#[tokio::test]
async fn a_federation_moderator_reaches_peers_without_being_admin() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    add_user(&ctx, "carol", false).await;
    gitquarry_core::auth::domain::user::toggle_federation_moderator(&ctx, "carol", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let (session, _csrf) = login(&app, "carol").await;
    let response = get(&app, "/_peers", Some(&session)).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(received(&mut rx).header("x-gitquarry-user"), Some("carol"));
}

/// What federation needs to render `/_peers` for this session: who it is,
/// and the token core's own CSRF check will accept back — never the session
/// cookie behind it.
#[tokio::test]
async fn peers_asserts_the_session_s_user_and_csrf_token_to_federation() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    add_user(&ctx, "alice", true).await;
    let (session, csrf) = login(&app, "alice").await;
    let request = Request::builder()
        .uri("/_peers")
        .header(header::COOKIE, &session)
        .header("x-gitquarry-user", "mallory")
        .body(Body::empty())
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let got = received(&mut rx);
    assert_eq!(got.method, "GET");
    assert_eq!(got.uri, "/_peers");
    assert_signed(&got);
    assert_eq!(got.header("x-gitquarry-user"), Some("alice"));
    assert_eq!(got.headers.get_all("x-gitquarry-user").iter().count(), 1);
    assert_eq!(got.header("x-gitquarry-csrf"), Some(csrf.as_str()));
    let session_value = session.strip_prefix("web-session=").unwrap();
    assert_ne!(csrf, session_value);
    assert!(got.headers.get("cookie").is_none());
    assert!(got.headers.get("x-gitquarry-user-uid").is_none());
}

/// Federation renders the token but never validates it — it has no session
/// to validate against — so core has to, before the mutation leaves.
#[tokio::test]
async fn a_peers_mutation_is_forwarded_only_with_this_session_s_csrf_token() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    add_user(&ctx, "alice", true).await;
    let (session, csrf) = login(&app, "alice").await;
    let session_value = session.strip_prefix("web-session=").unwrap();

    for body in [
        "action=ban_peer&name=evil".to_string(),
        "action=ban_peer&name=evil&csrf=wrong".to_string(),
        format!("action=ban_peer&name=evil&csrf={}", url_encode(session_value)),
    ] {
        let response = post_form(&app, "/_peers", Some(&session), &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert_nothing_forwarded(&mut rx, &body);
    }

    let body = format!("action=ban_peer&csrf={csrf}&name=evil");
    let response = post_form(&app, "/_peers", Some(&session), &body).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let got = received(&mut rx);
    assert_eq!(got.method, "POST");
    assert_eq!(
        &got.body[..],
        body.as_bytes(),
        "the form must ride through byte-for-byte"
    );
    assert_eq!(got.header("content-type"), Some("application/x-www-form-urlencoded"));
}

/// Federation's credential storage is keyed by uid, so `/_account/tokens`
/// also asserts that — always the session's own, never a client's claim.
#[tokio::test]
async fn account_tokens_asserts_the_session_s_own_uid() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    add_user(&ctx, "bob", false).await;
    let uid = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "bob")
        .await
        .unwrap()
        .0
        .unwrap()
        .uid
        .to_string();

    let response = get(&app, "/_account/tokens", None).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_nothing_forwarded(&mut rx, "an anonymous /_account/tokens");

    let (session, csrf) = login(&app, "bob").await;
    let request = Request::builder()
        .uri("/_account/tokens")
        .header(header::COOKIE, &session)
        .header("x-gitquarry-user-uid", "someone-elses-uid")
        .body(Body::empty())
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let got = received(&mut rx);
    assert_eq!(got.uri, "/_account/tokens");
    assert_signed(&got);
    assert_eq!(got.header("x-gitquarry-user"), Some("bob"));
    assert_eq!(got.header("x-gitquarry-user-uid"), Some(uid.as_str()));
    assert_eq!(got.headers.get_all("x-gitquarry-user-uid").iter().count(), 1);
    assert_eq!(got.header("x-gitquarry-csrf"), Some(csrf.as_str()));

    let response = post_form(&app, "/_account/tokens", Some(&session), "action=revoke&id=1").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_nothing_forwarded(&mut rx, "a tokens mutation without csrf");
    let body = format!("action=revoke&id=1&csrf={csrf}");
    let response = post_form(&app, "/_account/tokens", Some(&session), &body).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(&received(&mut rx).body[..], body.as_bytes());
}

#[tokio::test]
async fn a_remote_comment_is_csrf_checked_here_and_re_encoded_without_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    add_user(&ctx, "bob", false).await;
    let uid = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "bob")
        .await
        .unwrap()
        .0
        .unwrap()
        .uid
        .to_string();
    let form = "peer=instance-b&repo=acme%2Fdemo&number=7&body=hi+there";

    let response = post_form(&app, "/_remote_pulls/comment", None, form).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_nothing_forwarded(&mut rx, "an anonymous comment");

    let (session, csrf) = login(&app, "bob").await;
    let response = post_form(
        &app,
        "/_remote_pulls/comment",
        Some(&session),
        &format!("{form}&csrf=wrong"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_nothing_forwarded(&mut rx, "a comment with a bad csrf token");

    let response = post_form(
        &app,
        "/_remote_pulls/comment",
        Some(&session),
        &format!("{form}&csrf={csrf}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let got = received(&mut rx);
    assert_eq!(got.method, "POST");
    assert_eq!(got.uri, "/_remote_pulls/comment");
    assert_signed(&got);
    assert_eq!(got.header("content-type"), Some("application/json"));
    assert_eq!(got.header("x-gitquarry-user-uid"), Some(uid.as_str()));
    let json: serde_json::Value = serde_json::from_slice(&got.body).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "peer": "instance-b", "repo": "acme/demo", "number": 7, "body": "hi there" })
    );
}

fn url_encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
