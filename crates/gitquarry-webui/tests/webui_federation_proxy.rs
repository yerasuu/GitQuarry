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
use gitquarry_test_support::{Page, Session, ctx, user};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

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

fn federated(root: &std::path::Path, base_url: String) -> AppContext {
    AppContext {
        federation: Some(FederationLink {
            base_url,
            internal_token: INTERNAL_TOKEN.to_string(),
            http: reqwest::Client::new(),
        }),
        ..ctx(root)
    }
}

/// A core wired to a fresh fake federation.
async fn wired(root: &std::path::Path) -> (Arc<AppContext>, axum::Router, UnboundedReceiver<Received>) {
    let (url, rx) = spawn_fake_federation().await;
    let ctx = Arc::new(federated(root, url));
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    (ctx, app, rx)
}

/// Through the shared `send`, but from `CLIENT_IP` rather than its
/// loopback default, so the proxy's `X-Forwarded-For` is told apart from
/// anything a client claims.
async fn send(app: &axum::Router, mut request: Request<Body>) -> Page {
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((CLIENT_IP, 4242))));
    gitquarry_test_support::send(app, request).await
}

async fn get(app: &axum::Router, path: &str, cookie: Option<&str>) -> Page {
    let mut builder = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::empty()).unwrap()).await
}

/// A raw form body, so a test can send a missing or wrong CSRF token that
/// `Session::post` would never produce.
async fn post_form(app: &axum::Router, path: &str, cookie: Option<&str>, body: &str) -> Page {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::from(body.to_string())).unwrap()).await
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
    assert_eq!(response.status, StatusCode::CREATED);

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
    assert_eq!(response.status, StatusCode::CREATED);
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
    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(response.headers["x-federation-marker"], "relayed");
    assert!(response.headers.get("keep-alive").is_none());
    assert!(response.headers.get(header::CONNECTION).is_none());
    assert_eq!(response.body, "from federation");
}

#[tokio::test]
async fn an_unreachable_federation_is_a_bad_gateway() {
    let dir = tempfile::tempdir().unwrap();
    // Port 1: nothing listens there, so the connect is refused outright.
    let app = gitquarry_webui::webui::router(Arc::new(federated(dir.path(), "http://127.0.0.1:1".to_string())));
    let response = get(&app, "/_api/instance", None).await;
    assert_eq!(response.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn every_federation_route_is_unavailable_without_federation() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    for path in [
        "/_api/instance",
        "/_api/repos/team/project",
        "/_federation/gossip/peers",
    ] {
        let response = get(&app, path, None).await;
        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE, "GET {path}");
    }
    // `/_peers` is admin-gated first, so it only reports that to an admin.
    user(&ctx, "alice", true).await;
    let session = Session::login(&app, "alice").await.cookie;
    let response = get(&app, "/_peers", Some(&session)).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
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
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "GET {path}");
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
        assert_eq!(response.status, StatusCode::CREATED, "GET {path}");
        assert_eq!(received(&mut rx).uri, path);
    }
}

#[tokio::test]
async fn peers_is_gated_before_anything_is_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    user(&ctx, "bob", false).await;

    let response = get(&app, "/_peers", None).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/_login?return_to=/_peers");
    assert_nothing_forwarded(&mut rx, "an anonymous /_peers");

    let session = Session::login(&app, "bob").await.cookie;
    let response = get(&app, "/_peers", Some(&session)).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_nothing_forwarded(&mut rx, "a non-admin's /_peers");
}

/// Before any login method exists no session can, so the only credential
/// that opens an admin page is the bootstrap grant — `/_peers` included.
#[tokio::test]
async fn a_fresh_instance_keeps_peers_closed_without_the_bootstrap_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, mut rx) = wired(dir.path()).await;

    let response = get(&app, "/_peers", None).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_nothing_forwarded(&mut rx, "an anonymous /_peers on a fresh instance");
}

#[tokio::test]
async fn a_federation_moderator_reaches_peers_without_being_admin() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    user(&ctx, "carol", false).await;
    gitquarry_core::auth::domain::user::toggle_federation_moderator(&ctx, "carol", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let session = Session::login(&app, "carol").await.cookie;
    let response = get(&app, "/_peers", Some(&session)).await;
    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(received(&mut rx).header("x-gitquarry-user"), Some("carol"));
}

/// What federation needs to render `/_peers` for this session: who it is,
/// and the token core's own CSRF check will accept back — never the session
/// cookie behind it.
#[tokio::test]
async fn peers_asserts_the_session_s_user_and_csrf_token_to_federation() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    user(&ctx, "alice", true).await;
    let Session { cookie: session, csrf } = Session::login(&app, "alice").await;
    let request = Request::builder()
        .uri("/_peers")
        .header(header::COOKIE, &session)
        .header("x-gitquarry-user", "mallory")
        .body(Body::empty())
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::CREATED);

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
    user(&ctx, "alice", true).await;
    let Session { cookie: session, csrf } = Session::login(&app, "alice").await;
    let session_value = session.strip_prefix("web-session=").unwrap();

    for body in [
        "action=ban_peer&name=evil".to_string(),
        "action=ban_peer&name=evil&csrf=wrong".to_string(),
        format!("action=ban_peer&name=evil&csrf={}", url_encode(session_value)),
    ] {
        let response = post_form(&app, "/_peers", Some(&session), &body).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        assert_nothing_forwarded(&mut rx, &body);
    }

    let body = format!("action=ban_peer&csrf={csrf}&name=evil");
    let response = post_form(&app, "/_peers", Some(&session), &body).await;
    assert_eq!(response.status, StatusCode::CREATED);
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
    user(&ctx, "bob", false).await;
    let uid = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "bob")
        .await
        .unwrap()
        .0
        .unwrap()
        .uid
        .to_string();

    let response = get(&app, "/_account/tokens", None).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_nothing_forwarded(&mut rx, "an anonymous /_account/tokens");

    let Session { cookie: session, csrf } = Session::login(&app, "bob").await;
    let request = Request::builder()
        .uri("/_account/tokens")
        .header(header::COOKIE, &session)
        .header("x-gitquarry-user-uid", "someone-elses-uid")
        .body(Body::empty())
        .unwrap();
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::CREATED);
    let got = received(&mut rx);
    assert_eq!(got.uri, "/_account/tokens");
    assert_signed(&got);
    assert_eq!(got.header("x-gitquarry-user"), Some("bob"));
    assert_eq!(got.header("x-gitquarry-user-uid"), Some(uid.as_str()));
    assert_eq!(got.headers.get_all("x-gitquarry-user-uid").iter().count(), 1);
    assert_eq!(got.header("x-gitquarry-csrf"), Some(csrf.as_str()));

    let response = post_form(&app, "/_account/tokens", Some(&session), "action=revoke&id=1").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_nothing_forwarded(&mut rx, "a tokens mutation without csrf");
    let body = format!("action=revoke&id=1&csrf={csrf}");
    let response = post_form(&app, "/_account/tokens", Some(&session), &body).await;
    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(&received(&mut rx).body[..], body.as_bytes());
}

#[tokio::test]
async fn a_remote_comment_is_csrf_checked_here_and_re_encoded_without_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, mut rx) = wired(dir.path()).await;
    user(&ctx, "bob", false).await;
    let uid = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "bob")
        .await
        .unwrap()
        .0
        .unwrap()
        .uid
        .to_string();
    let form = "peer=instance-b&repo=acme%2Fdemo&number=7&body=hi+there";

    let response = post_form(&app, "/_remote_pulls/comment", None, form).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_nothing_forwarded(&mut rx, "an anonymous comment");

    let Session { cookie: session, csrf } = Session::login(&app, "bob").await;
    let response = post_form(
        &app,
        "/_remote_pulls/comment",
        Some(&session),
        &format!("{form}&csrf=wrong"),
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_nothing_forwarded(&mut rx, "a comment with a bad csrf token");

    let response = post_form(
        &app,
        "/_remote_pulls/comment",
        Some(&session),
        &format!("{form}&csrf={csrf}"),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED);
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
