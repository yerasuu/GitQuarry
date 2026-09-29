//! `/_account/security`: how the logged-in account signs in — password,
//! two-factor and recovery codes — driven through `router()` with a real
//! session.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use gitquarry_test_support::{PASSWORD, Session, ctx, send, user};
use std::sync::Arc;

const TOTP_SECRET: &str = "JBSWY3DPEHPK3PXP";

async fn app_with(root: &std::path::Path, is_admin: bool) -> (Arc<AppContext>, axum::Router, Session) {
    let ctx = Arc::new(ctx(root));
    user(&ctx, "alice", is_admin).await;
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let session = Session::login(&app, "alice").await;
    (ctx, app, session)
}

async fn security_page(app: &axum::Router, session: &Session) -> String {
    let page = session.get(app, "/_account/security").await;
    assert_eq!(page.status, StatusCode::OK, "GET /_account/security");
    page.body
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

async fn submit(app: &axum::Router, session: &Session, fields: &[(&str, &str)]) -> String {
    let page = session.post(app, "/_account/security", fields).await;
    assert_eq!(page.status, StatusCode::OK);
    page.body
}

#[tokio::test]
async fn security_page_never_exposes_admin_or_delete_actions() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), true).await;
    domain::user::set_email(&ctx, "alice", Some("alice@example.com"), gitquarry_i18n::Locale::En)
        .await
        .unwrap();

    let html = security_page(&app, &session).await;
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

    // How `style/app.js` sends a `data-async` submit, which gets back just
    // the fragment its target names.
    let request = Request::post("/_account/security")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(Body::from(format!("csrf={}&security_action=enable_totp", session.csrf)))
        .unwrap();
    let fragment = send(&app, request).await.body;
    assert!(fragment.starts_with(r#"<div id="security-body">"#), "{fragment}");
}

#[tokio::test]
async fn totp_section_offers_activation_when_nothing_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with(dir.path(), false).await;

    let html = security_page(&app, &session).await;
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

    let html = security_page(&app, &session).await;
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
    let html = security_page(&app, &session).await;
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
    let page = session.get(&app, "/_account/security").await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location(), "/_login?return_to=/_account/security");
}

#[tokio::test]
async fn a_security_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with(dir.path(), false).await;

    let request = Request::post("/_account/security")
        .header(header::COOKIE, &session.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("csrf=forged&security_action=enable_totp"))
        .unwrap();
    assert_eq!(send(&app, request).await.status, StatusCode::BAD_REQUEST);
    assert!(alice(&ctx).await.0.totp_secret.is_none());
}
