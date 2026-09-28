//! `/_login`, `/_login_totp`, `/_login_oidc`, `/_callback` and `/_logout`,
//! driven as a browser would: real form posts through `router()`, the
//! anchor/session cookies taken from `Set-Cookie` and sent back, CSRF tokens
//! read out of the page that embeds them.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::{access, domain};
use gitquarry_core::crypto::CompressionStrategy;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";
/// `LoginAttempts`' `LOGIN_MAX_ATTEMPTS`: failures before the penalty starts.
const MAX_ATTEMPTS: usize = 5;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; s3db_crypto::SALT_LEN]).unwrap());
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
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// An OIDC client that never needs its provider: every assertion here stops
/// at the flow-cookie checks `/_callback` runs before any token exchange.
fn offline_oidc_client() -> gitquarry_auth_crypto::oidc::OidcClient {
    use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType, CoreSubjectIdentifierType};
    use openidconnect::{
        AuthUrl, ClientId, ClientSecret, EmptyAdditionalProviderMetadata, IssuerUrl, JsonWebKeySetUrl, RedirectUrl,
        ResponseTypes, TokenUrl,
    };
    let metadata = CoreProviderMetadata::new(
        IssuerUrl::new("https://idp.invalid".to_string()).unwrap(),
        AuthUrl::new("https://idp.invalid/authorize".to_string()).unwrap(),
        JsonWebKeySetUrl::new("https://idp.invalid/jwks".to_string()).unwrap(),
        vec![ResponseTypes::new(vec![CoreResponseType::Code])],
        vec![CoreSubjectIdentifierType::Public],
        vec![],
        EmptyAdditionalProviderMetadata {},
    );
    CoreClient::from_provider_metadata(
        metadata,
        ClientId::new("gitquarry".to_string()),
        Some(ClientSecret::new("secret".to_string())),
    )
    .set_token_uri(TokenUrl::new("https://idp.invalid/token".to_string()).unwrap())
    .set_redirect_uri(RedirectUrl::new("https://git.invalid/_callback".to_string()).unwrap())
}

async fn create_user(ctx: &AppContext, username: &str) {
    domain::user::create(
        ctx,
        domain::user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't add it, and its absence 500s before routing.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    app.clone().oneshot(request).await.unwrap()
}

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::get(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn post_form(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

async fn text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The full `Set-Cookie` line for `name`, attributes included.
fn set_cookie_line(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(str::to_string)
}

/// Just `name=value`, the shape a browser sends back in `Cookie`.
fn cookie_pair(headers: &HeaderMap, name: &str) -> String {
    let line = set_cookie_line(headers, name).unwrap_or_else(|| panic!("no Set-Cookie for {name}: {headers:?}"));
    line.split(';').next().unwrap().to_string()
}

fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

fn csrf_in(html: &str) -> String {
    let marker = r#"name="csrf" value=""#;
    let start = html.find(marker).unwrap_or_else(|| panic!("no csrf field in {html}")) + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

/// A fresh `/_login` visit: the anchor cookie it plants plus the token its
/// form carries.
async fn login_form(app: &axum::Router) -> (String, String) {
    let response = send(app, get("/_login", "")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let anchor = cookie_pair(response.headers(), "login-csrf");
    (anchor, csrf_in(&text(response).await))
}

async fn submit_login(app: &axum::Router, username: &str, password: &str, return_to: &str) -> Response {
    let (anchor, csrf) = login_form(app).await;
    send(
        app,
        post_form(
            "/_login",
            &anchor,
            &[
                ("csrf", &csrf),
                ("username", username),
                ("password", password),
                ("return_to", return_to),
            ],
        ),
    )
    .await
}

async fn log_in(app: &axum::Router, username: &str) -> String {
    let response = submit_login(app, username, PASSWORD, "/").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    cookie_pair(response.headers(), "web-session")
}

/// Whether `cookie` still opens an account page rather than bouncing to
/// `/_login` — the observable meaning of "this session is valid".
async fn session_is_live(app: &axum::Router, cookie: &str) -> bool {
    let response = send(app, get("/_account/security", cookie)).await;
    match response.status() {
        StatusCode::OK => true,
        StatusCode::SEE_OTHER => {
            assert!(location(&response).starts_with("/_login?return_to="));
            false
        }
        other => panic!("unexpected {other}"),
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn sealed_cookie(ctx: &AppContext, name: &str, json: &str) -> String {
    let sealed = ctx
        .cipher
        .seal(name, json.as_bytes(), CompressionStrategy::None)
        .unwrap();
    format!("{name}={}", B64.encode(sealed))
}

fn metrics_gauge(body: &str) -> &str {
    body.lines()
        .find_map(|l| l.strip_prefix("gitquarry_login_penalised_active "))
        .unwrap()
}

async fn app_with_alice(root: &std::path::Path) -> (Arc<AppContext>, axum::Router) {
    let ctx = test_ctx(root);
    create_user(&ctx, "alice").await;
    let ctx = Arc::new(ctx);
    (Arc::clone(&ctx), gitquarry_webui::webui::router(ctx))
}

#[tokio::test]
async fn login_page_plants_a_short_lived_httponly_csrf_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = app_with_alice(dir.path()).await;

    let response = send(&app, get("/_login", "")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let line = set_cookie_line(response.headers(), "login-csrf").unwrap();
    assert!(
        line.ends_with("; Path=/; HttpOnly; SameSite=Lax; Max-Age=600"),
        "{line}"
    );
    let html = text(response).await;
    assert!(html.contains(r#"action="/_login""#), "{html}");
    assert!(html.contains(r#"type="password" name="password""#), "{html}");

    // Behind a TLS-terminating proxy the cookie must refuse plain HTTP.
    let request = Request::get("/_login")
        .header("x-forwarded-proto", "https")
        .body(Body::empty())
        .unwrap();
    let response = send(&app, request).await;
    let line = set_cookie_line(response.headers(), "login-csrf").unwrap();
    assert!(line.contains("; Secure"), "{line}");
}

#[tokio::test]
async fn correct_password_issues_a_session_and_redirects_to_return_to() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = app_with_alice(dir.path()).await;

    let response = submit_login(&app, "alice", PASSWORD, "/team/proj?view=log").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/team/proj?view=log");
    let line = set_cookie_line(response.headers(), "web-session").unwrap();
    assert!(
        line.ends_with("; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800"),
        "{line}"
    );

    let session = cookie_pair(response.headers(), "web-session");
    let response = send(&app, get("/_account/security", &session)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("<strong>alice</strong>"));
}

#[tokio::test]
async fn a_foreign_return_to_falls_back_to_root() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = app_with_alice(dir.path()).await;

    // Protocol-relative `//evil.com` resolves as `https://evil.com`, and
    // WHATWG URL parsing turns `/\evil.com` into the same thing for http(s).
    for hostile in ["https://evil.com", "evil.com", "//evil.com", "/\\evil.com", ""] {
        let response = submit_login(&app, "alice", PASSWORD, hostile).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{hostile}");
        assert_eq!(location(&response), "/", "{hostile}");
    }

    // The landing page sanitizes too, so the hidden field it hands the form
    // never carries the hostile value forward in the first place.
    let response = send(&app, get("/_login?return_to=%2F%2Fevil.com", "")).await;
    let html = text(response).await;
    assert!(html.contains(r#"name="return_to" value="/""#), "{html}");
    assert!(!html.contains("evil.com"), "{html}");
}

#[tokio::test]
async fn a_wrong_password_or_unknown_user_rerenders_the_form_without_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = app_with_alice(dir.path()).await;

    for (username, password) in [("alice", "wrong"), ("nobody", PASSWORD)] {
        let response = submit_login(&app, username, password, "/").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{username}");
        assert!(set_cookie_line(response.headers(), "web-session").is_none());
        let html = text(response).await;
        // The same message and both fields flagged either way, so a guesser
        // can't tell which half was wrong; the username is kept for a retry.
        assert!(html.contains("wrong username or password"), "{html}");
        assert_eq!(html.matches(r#"class="field invalid""#).count(), 2, "{html}");
        assert!(
            html.contains(&format!(r#"name="username" size="30" value="{username}""#)),
            "{html}"
        );
        assert!(html.contains(r#"name="csrf" value=""#), "{html}");
    }
}

#[tokio::test]
async fn a_login_post_without_its_csrf_anchor_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = app_with_alice(dir.path()).await;
    let fields = [("username", "alice"), ("password", PASSWORD), ("csrf", "forged")];

    let response = send(&app, post_form("/_login", "", &fields)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(response.headers(), "web-session").is_none());

    // A real anchor doesn't help a token that wasn't derived from it.
    let (anchor, _csrf) = login_form(&app).await;
    let response = send(&app, post_form("/_login", &anchor, &fields)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(response.headers(), "web-session").is_none());
}

#[tokio::test]
async fn repeated_failures_are_throttled_until_a_successful_login() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = app_with_alice(dir.path()).await;

    for _ in 0..MAX_ATTEMPTS {
        let response = submit_login(&app, "alice", "wrong", "/").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let metrics = text(send(&app, get("/_metrics", "")).await).await;
    assert_eq!(metrics_gauge(&metrics), "1", "{metrics}");
    assert_eq!(ctx.login_attempts.penalty("alice"), std::time::Duration::from_secs(1));
    // Keyed per username: a different account isn't slowed by alice's.
    assert!(ctx.login_attempts.penalty("bob").is_zero());

    // Slowed, not locked: the real owner still gets in, and that clears it.
    log_in(&app, "alice").await;
    let metrics = text(send(&app, get("/_metrics", "")).await).await;
    assert_eq!(metrics_gauge(&metrics), "0", "{metrics}");
    assert!(ctx.login_attempts.penalty("alice").is_zero());
}

#[tokio::test]
async fn logout_needs_the_session_csrf_token_and_then_clears_the_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app) = app_with_alice(dir.path()).await;
    let session = log_in(&app, "alice").await;

    let response = send(&app, post_form("/_logout", &session, &[("csrf", "forged")])).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(response.headers(), "web-session").is_none());

    // The token the topbar's own logout form carries.
    let page = text(send(&app, get("/", &session)).await).await;
    assert!(page.contains(r#"action="/_logout""#), "{page}");
    let response = send(&app, post_form("/_logout", &session, &[("csrf", &csrf_in(&page))])).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/");
    let line = set_cookie_line(response.headers(), "web-session").unwrap();
    assert!(line.starts_with("web-session=; "), "{line}");
    assert!(line.ends_with("; Max-Age=0"), "{line}");
}

/// A cookie sealed before `epoch` existed must still open a session, as
/// epoch `0` — that is what kept live sessions valid across the upgrade
/// that added the field, since stored `User` records read as `0` too.
#[tokio::test]
async fn a_session_cookie_without_an_epoch_still_logs_in_as_epoch_zero() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = app_with_alice(dir.path()).await;
    let exp = now_secs() + 3600;

    let legacy = sealed_cookie(
        &ctx,
        "web-session",
        &format!(r#"{{"subject":{{"Username":"alice"}},"exp":{exp}}}"#),
    );
    assert!(session_is_live(&app, &legacy).await);

    // Its own `exp` is the deadline, whatever `Max-Age` the browser ignored.
    let expired = sealed_cookie(
        &ctx,
        "web-session",
        &format!(r#"{{"subject":{{"Username":"alice"}},"exp":{}}}"#, now_secs() - 1),
    );
    assert!(!session_is_live(&app, &expired).await);

    // A password change bumps the stored epoch, revoking the old cookie.
    domain::user::set_password(
        &ctx,
        "alice",
        Some("a brand new passphrase"),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(!session_is_live(&app, &legacy).await);
}

async fn enable_totp_with_recovery_code(ctx: &AppContext, username: &str, code: &str) {
    let (user, etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap();
    let mut user = user.unwrap();
    user.totp_secret = Some(gitquarry_auth_crypto::totp::generate_secret());
    user.totp_enabled = true;
    user.totp_recovery_codes = vec![gitquarry_auth_crypto::password::hash_password(code).unwrap()];
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_totp_account_gets_a_pending_step_and_no_session_until_the_code_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = app_with_alice(dir.path()).await;
    enable_totp_with_recovery_code(&ctx, "alice", "aaaa-bbbb-cccc-dddd").await;

    let response = submit_login(&app, "alice", PASSWORD, "/team/proj").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/_login_totp");
    assert!(set_cookie_line(response.headers(), "web-session").is_none());
    let line = set_cookie_line(response.headers(), "pending-totp").unwrap();
    assert!(line.ends_with("; HttpOnly; SameSite=Lax; Max-Age=300"), "{line}");
    let pending = cookie_pair(response.headers(), "pending-totp");

    let page = text(send(&app, get("/_login_totp", &pending)).await).await;
    assert!(page.contains(r#"action="/_login_totp""#), "{page}");
    let csrf = csrf_in(&page);

    let response = send(
        &app,
        post_form(
            "/_login_totp",
            &pending,
            &[("csrf", "forged"), ("code", "aaaa-bbbb-cccc-dddd")],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(response.headers(), "web-session").is_none());

    let response = send(
        &app,
        post_form("/_login_totp", &pending, &[("csrf", &csrf), ("code", "not-a-code")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(response.headers(), "web-session").is_none());

    let response = send(
        &app,
        post_form(
            "/_login_totp",
            &pending,
            &[("csrf", &csrf), ("code", "aaaa-bbbb-cccc-dddd")],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/team/proj");
    let session = cookie_pair(response.headers(), "web-session");
    let cleared = set_cookie_line(response.headers(), "pending-totp").unwrap();
    assert!(
        cleared.starts_with("pending-totp=; ") && cleared.ends_with("; Max-Age=0"),
        "{cleared}"
    );
    assert!(session_is_live(&app, &session).await);

    // A recovery code is single-use, even replayed with the same pending cookie.
    let response = send(
        &app,
        post_form(
            "/_login_totp",
            &pending,
            &[("csrf", &csrf), ("code", "aaaa-bbbb-cccc-dddd")],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(response.headers(), "web-session").is_none());
}

#[tokio::test]
async fn the_totp_step_refuses_a_missing_or_expired_pending_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app) = app_with_alice(dir.path()).await;

    let response = send(&app, get("/_login_totp", "")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/_login");

    // Sealed before `exp` existed: reads as `0`, already past, so it fails
    // closed rather than being a "password already matched" claim forever.
    let legacy = sealed_cookie(&ctx, "pending-totp", r#"{"username":"alice","return_to":"/"}"#);
    let csrf = csrf_in(&text(send(&app, get("/_login_totp", &legacy)).await).await);
    let response = send(
        &app,
        post_form("/_login_totp", &legacy, &[("csrf", &csrf), ("code", "123456")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(text(response).await.contains("no login in progress"));
}

#[tokio::test]
async fn the_oidc_callback_checks_its_flow_cookie_before_any_token_exchange() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = test_ctx(dir.path());
    ctx.oidc = Some(offline_oidc_client());
    let ctx = Arc::new(ctx);
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));

    let response = send(&app, get("/_login_oidc?return_to=%2F%2Fevil.com", "")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location(&response).starts_with("https://idp.invalid/authorize?"));
    let line = set_cookie_line(response.headers(), "oidc-flow").unwrap();
    assert!(line.ends_with("; HttpOnly; SameSite=Lax; Max-Age=600"), "{line}");
    let flow = cookie_pair(response.headers(), "oidc-flow");

    let response = send(&app, get("/_callback?code=c&state=s", "")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // The IdP's `state` has to echo the one this browser's flow started with.
    let response = send(&app, get("/_callback?code=c&state=forged", &flow)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(text(response).await.contains("state mismatch"));

    // Sealed before `exp` existed: already expired, even with a matching state.
    let legacy = sealed_cookie(
        &ctx,
        "oidc-flow",
        r#"{"csrf":"s","nonce":"n","pkce_verifier":"v","return_to":"/"}"#,
    );
    let response = send(&app, get("/_callback?code=c&state=s", &legacy)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(response.headers(), "web-session").is_none());
}
