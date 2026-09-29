//! `/_account/keys`: the logged-in account's SSH and GPG keys, split out of
//! `security` along with the section itself.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_test_support::{PASSWORD, Session, ctx, send, user};
use std::sync::Arc;

/// A form post as `cookie`, carrying exactly `fields` — including whatever
/// `csrf` the test chooses to send.
fn form(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

/// The page at `uri` for `session`, which must render.
async fn page(app: &axum::Router, session: &Session, uri: &str) -> String {
    let page = session.get(app, uri).await;
    assert_eq!(page.status, StatusCode::OK, "GET {uri}");
    page.body
}

const PUBKEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK787z0ABFAXS4w2j6x8H+c/+hhOEFYZwauXxV9nstMy alice@laptop";

async fn app_with_alice(root: &std::path::Path) -> (Arc<AppContext>, axum::Router, Session) {
    let ctx = ctx(root);
    user(&ctx, "alice", false).await;
    let ctx = Arc::new(ctx);
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let session = Session::login(&app, "alice").await;
    (ctx, app, session)
}

async fn stored_pubkeys(ctx: &AppContext) -> Vec<String> {
    let (user, _etag) = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    user.unwrap().pubkeys
}

/// Regression: `add_pubkey`/`add_gpg_key` each target their own table's
/// `id`, same as the `remove_*` forms — a page-level fragment swapped into
/// a table-only target would corrupt the DOM. Losing either attribute
/// quietly brings back the browser's "confirm resubmission" dialog on F5.
#[tokio::test]
async fn add_forms_target_their_own_table() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_alice(dir.path()).await;

    let html = page(&app, &session, "/_account/keys").await;
    assert!(
        html.contains(r##"data-async data-async-target="#ssh-keys-table""##),
        "{html}"
    );
    assert!(
        html.contains(r##"data-async data-async-target="#gpg-keys-table""##),
        "{html}"
    );
}

#[tokio::test]
async fn gpg_keys_section_shows_hint_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_alice(dir.path()).await;

    let html = page(&app, &session, "/_account/keys").await;
    assert!(html.contains("no keys yet"), "{html}");
    assert!(html.contains(r#"value="add_gpg_key""#));
}

#[tokio::test]
async fn gpg_keys_section_lists_each_stored_key_by_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_alice(dir.path()).await;
    let (user, etag) = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let mut user = user.unwrap();
    user.gpg_keys = vec![gitquarry_core::auth::entities::GpgKeyEntry {
        armored: "totally-fake-armored-key-body-xyz".to_string(),
        fingerprint: "ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234".to_string(),
    }];
    gitquarry_core::auth::access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref())
        .await
        .unwrap();

    let html = page(&app, &session, "/_account/keys").await;
    assert!(html.contains("ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234"), "{html}");
    assert!(html.contains(r#"value="remove_gpg_key""#));
    // The raw armored block itself never gets re-rendered — only the
    // already-computed fingerprint, same as the SSH table never echoes the
    // raw pubkey line back either.
    assert!(!html.contains("totally-fake-armored-key-body-xyz"), "{html}");
}

/// A stolen session cookie alone must not be able to plant an SSH key that
/// then pushes on its own: `add_pubkey` re-proves the account password.
#[tokio::test]
async fn adding_an_ssh_key_needs_the_account_password() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_alice(dir.path()).await;
    let html = page(&app, &session, "/_account/keys").await;
    assert!(html.contains(r#"name="reauth""#), "{html}");

    for reauth in ["", "not my password"] {
        let fields = [("keys_action", "add_pubkey"), ("pubkey", PUBKEY), ("reauth", reauth)];
        let response = session.post(&app, "/_account/keys", &fields).await;
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.body.contains("wrong password or code"));
    }
    assert!(stored_pubkeys(&ctx).await.is_empty());

    let fields = [
        ("csrf", session.csrf.as_str()),
        ("keys_action", "add_pubkey"),
        ("pubkey", PUBKEY),
        ("reauth", PASSWORD),
    ];
    let mut request = form("/_account/keys", &session.cookie, &fields);
    // `style/app.js` marks a `data-async` submit this way, which gets back
    // just the fragment its target names.
    request.headers_mut().insert("x-gq-async", "1".parse().unwrap());
    let response = send(&app, request).await;
    assert_eq!(response.status, StatusCode::OK);
    let fragment = response.body;
    assert!(fragment.starts_with(r#"<div id="ssh-keys-table">"#), "{fragment}");
    let fingerprint = gitquarry_core::auth::access::key_fingerprint(PUBKEY).unwrap();
    assert!(fragment.contains(&fingerprint), "{fragment}");
    assert_eq!(stored_pubkeys(&ctx).await.len(), 1);
}

#[tokio::test]
async fn a_keys_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_alice(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("keys_action", "add_pubkey"),
        ("pubkey", PUBKEY),
        ("reauth", PASSWORD),
    ];
    let response = send(&app, form("/_account/keys", &session.cookie, &fields)).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(stored_pubkeys(&ctx).await.is_empty());

    let response = send(&app, form("/_account/keys", "", &fields)).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/_login?return_to=/_account/keys");
}
