//! `/_account/security`: how the logged-in account signs in — password,
//! two-factor and recovery codes — driven through `router()` with a real
//! session.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[SALT_BYTE; s3db_crypto::SALT_LEN]).unwrap(),
    );
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

async fn create_user(ctx: &AppContext, username: &str, is_admin: bool) {
    domain::user::create(
        ctx,
        domain::user::NewUser {
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

/// [`post_form`] as `style/app.js` sends a `data-async` submit, which gets
/// back just the fragment its target names.
fn post_async(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let mut request = post_form(uri, cookie, fields);
    request.headers_mut().insert("x-gq-async", "1".parse().unwrap());
    request
}

async fn text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// `name=value` out of the response's `Set-Cookie`, the shape a browser
/// sends back in `Cookie`.
fn cookie_pair(response: &Response, name: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .and_then(|v| v.split(';').next())
        .unwrap_or_else(|| panic!("no Set-Cookie for {name}"))
        .to_string()
}

fn csrf_in(html: &str) -> String {
    let marker = r#"name="csrf" value=""#;
    let start = html.find(marker).unwrap_or_else(|| panic!("no csrf field in {html}")) + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

/// A real password login, returning the session cookie.
async fn log_in(app: &axum::Router, username: &str) -> String {
    let response = send(app, get("/_login", "")).await;
    let anchor = cookie_pair(&response, "login-csrf");
    let csrf = csrf_in(&text(response).await);
    let fields = [("csrf", csrf.as_str()), ("username", username), ("password", PASSWORD)];
    let response = send(app, post_form("/_login", &anchor, &fields)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    cookie_pair(&response, "web-session")
}

/// The page at `uri` for `cookie`'s session, plus the CSRF token its forms
/// carry — read out of the markup the way a browser submits it.
async fn page(app: &axum::Router, uri: &str, cookie: &str) -> (String, String) {
    let response = send(app, get(uri, cookie)).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
    let html = text(response).await;
    let csrf = csrf_in(&html);
    (html, csrf)
}

fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

const SALT_BYTE: u8 = 16;
const TOTP_SECRET: &str = "JBSWY3DPEHPK3PXP";

async fn app_with(root: &std::path::Path, is_admin: bool) -> (Arc<AppContext>, axum::Router, String) {
    let ctx = test_ctx(root);
    create_user(&ctx, "alice", is_admin).await;
    let ctx = Arc::new(ctx);
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let session = log_in(&app, "alice").await;
    (ctx, app, session)
}

async fn alice(ctx: &AppContext) -> (gitquarry_core::auth::entities::User, Option<String>) {
    let (user, etag) = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    (user.unwrap(), etag)
}

/// Turns 2FA on straight in the store, since enrolling through the page
/// needs a live code for a secret only the page knows. Leaves
/// `session_epoch` alone, unlike `confirm_totp`, so the test's session
/// survives it.
async fn enable_totp(ctx: &AppContext) {
    let (mut user, etag) = alice(ctx).await;
    user.totp_secret = Some(TOTP_SECRET.to_string());
    user.totp_enabled = true;
    gitquarry_core::auth::access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref())
        .await
        .unwrap();
}

async fn submit(app: &axum::Router, session: &str, fields: &[(&str, &str)]) -> String {
    let (_html, csrf) = page(app, "/_account/security", session).await;
    let mut all = vec![("csrf", csrf.as_str())];
    all.extend_from_slice(fields);
    let response = send(app, post_form("/_account/security", session, &all)).await;
    assert_eq!(response.status(), StatusCode::OK);
    text(response).await
}

#[tokio::test]
async fn security_page_never_exposes_admin_or_delete_actions() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), true).await;
    domain::user::set_email(&ctx, "alice", Some("alice@example.com"), gitquarry_i18n::Locale::En)
        .await
        .unwrap();

    let (html, _csrf) = page(&app, "/_account/security", &session).await;
    // No self-service toggle for the flag that grants /_settings + /_users.
    assert!(!html.contains("toggle_admin"), "{html}");
    assert!(!html.contains("delete_user"), "{html}");
    // Regression: every form on this page (set_password here, plus
    // whichever 2FA form the current state shows) targets one
    // `#security-body` wrapper — a submit can reveal a TOTP secret/QR or a
    // recovery-code set only the *next* render shows, so it has to refresh
    // the whole page, not just its own form. Losing this quietly brings
    // back the browser's "confirm resubmission" dialog on F5.
    assert_eq!(
        html.matches(r##"data-async data-async-target="#security-body""##)
            .count(),
        2,
        "{html}"
    );
    // Own identity is shown but never editable here: the address moved to
    // `/_account/profile` entirely, and the only `username` control left is
    // the hidden one a password manager reads to know which account the
    // new password belongs to.
    assert!(html.contains("alice@example.com"));
    assert!(!html.contains(r#"name="email""#));
    assert!(!html.contains(r#"<input name="username""#));
    assert!(
        html.contains(r#"input type="hidden" name="username" value="alice""#),
        "{html}"
    );

    let (_html, csrf) = page(&app, "/_account/security", &session).await;
    let fields = [("csrf", csrf.as_str()), ("security_action", "enable_totp")];
    let fragment = text(send(&app, post_async("/_account/security", &session, &fields)).await).await;
    assert!(fragment.starts_with(r#"<div id="security-body">"#), "{fragment}");
}

#[tokio::test]
async fn totp_section_offers_activation_when_nothing_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with(dir.path(), false).await;

    let (html, _csrf) = page(&app, "/_account/security", &session).await;
    assert!(html.contains(r#"value="enable_totp""#));
    assert!(!html.contains(r#"value="confirm_totp""#));
    assert!(!html.contains(r#"value="disable_totp""#));
    // Nothing to lose yet, so nothing to recover.
    assert!(!html.contains(r#"value="regenerate_recovery_codes""#));
}

#[tokio::test]
async fn enabling_totp_shows_the_pending_secret_and_a_confirm_form() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), false).await;

    let html = submit(&app, &session, &[("security_action", "enable_totp")]).await;
    let (user, _etag) = alice(&ctx).await;
    let secret = user.totp_secret.unwrap();
    assert!(html.contains(&format!("<code>{secret}</code>")), "{html}");
    assert!(html.contains(r#"value="confirm_totp""#));
    assert!(!html.contains(r#"value="enable_totp""#));
    assert!(!html.contains(r#"value="disable_totp""#));

    // Pending until a code proves the authenticator actually has it.
    let html = submit(
        &app,
        &session,
        &[("security_action", "confirm_totp"), ("totp_code", "not-a-code")],
    )
    .await;
    assert!(html.contains("incorrect code"), "{html}");
    assert!(!alice(&ctx).await.0.totp_enabled);
}

#[tokio::test]
async fn totp_section_offers_disable_once_enabled_and_hides_enable_form() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), false).await;
    enable_totp(&ctx).await;

    let (html, _csrf) = page(&app, "/_account/security", &session).await;
    assert!(html.contains(r#"value="disable_totp""#));
    assert!(!html.contains(r#"value="enable_totp""#));
    assert!(!html.contains(r#"value="confirm_totp""#));
    // The secret itself is never re-rendered once enabled.
    assert!(!html.contains(TOTP_SECRET));
}

/// A stolen session cookie alone must not turn 2FA off: the account
/// password (or a live code) is asked for again, and a wrong guess counts
/// against the same per-username limiter `/_login` uses.
#[tokio::test]
async fn disabling_totp_needs_the_password_and_counts_failures() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), false).await;
    enable_totp(&ctx).await;

    for _ in 0..5 {
        let html = submit(
            &app,
            &session,
            &[("security_action", "disable_totp"), ("totp_code", "guess")],
        )
        .await;
        assert!(html.contains("wrong password or code"), "{html}");
    }
    assert!(alice(&ctx).await.0.totp_enabled);
    assert!(!ctx.login_attempts.penalty("alice").is_zero());

    submit(
        &app,
        &session,
        &[("security_action", "disable_totp"), ("totp_code", PASSWORD)],
    )
    .await;
    assert!(!alice(&ctx).await.0.totp_enabled);
    assert!(ctx.login_attempts.penalty("alice").is_zero());
}

#[tokio::test]
async fn recovery_codes_are_shown_once_then_never_again() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), false).await;
    enable_totp(&ctx).await;

    let html = submit(
        &app,
        &session,
        &[
            ("security_action", "regenerate_recovery_codes"),
            ("totp_code", PASSWORD),
        ],
    )
    .await;
    let start = html.find("<pre>").unwrap_or_else(|| panic!("no codes in {html}")) + "<pre>".len();
    let codes: Vec<&str> = html[start..start + html[start..].find("</pre>").unwrap()]
        .lines()
        .collect();
    assert_eq!(codes.len(), 10, "{html}");
    assert!(html.contains(r#"value="regenerate_recovery_codes""#));

    // Without a fresh reveal, the (never-stored) plaintext just isn't there.
    let (html, _csrf) = page(&app, "/_account/security", &session).await;
    for code in codes {
        assert!(!html.contains(code), "{html}");
    }
    assert!(html.contains(r#"value="regenerate_recovery_codes""#));
}

#[tokio::test]
async fn changing_the_password_needs_the_old_one_and_revokes_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with(dir.path(), false).await;
    let new = "a brand new passphrase";

    let html = submit(
        &app,
        &session,
        &[
            ("security_action", "set_password"),
            ("password", new),
            ("password_confirm", new),
            ("reauth", "guess"),
        ],
    )
    .await;
    assert!(html.contains("wrong password or code"), "{html}");

    let html = submit(
        &app,
        &session,
        &[
            ("security_action", "set_password"),
            ("password", new),
            ("password_confirm", "a brand new typo"),
            ("reauth", PASSWORD),
        ],
    )
    .await;
    assert!(html.contains("the two passwords don"), "{html}");

    submit(
        &app,
        &session,
        &[
            ("security_action", "set_password"),
            ("password", new),
            ("password_confirm", new),
            ("reauth", PASSWORD),
        ],
    )
    .await;
    // Every session issued before the change goes, this one included.
    let response = send(&app, get("/_account/security", &session)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/_login?return_to=/_account/security");
}

#[tokio::test]
async fn a_security_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), false).await;

    let fields = [("csrf", "forged"), ("security_action", "enable_totp")];
    let response = send(&app, post_form("/_account/security", &session, &fields)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(alice(&ctx).await.0.totp_secret.is_none());
}
