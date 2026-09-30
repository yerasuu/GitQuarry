//! Passkeys through `webui::router`: the public URL they hang off, the
//! registration ceremony on `/_account/security`, passkey-only sign-in from
//! `/_login` and the passkey as a second factor from `/_login_totp`, every
//! response produced by the shared software authenticator.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use gitquarry_auth_crypto::webauthn::{self, RelyingParty};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::{access, domain};
use gitquarry_core::crypto::CompressionStrategy;
use gitquarry_core::storage;
use gitquarry_test_support::authenticator::{Authenticator, Key, UP, UV};
use gitquarry_test_support::{PASSWORD, Page, Session, ctx, post_form, send, user};
use serde_json::{Value, json};
use std::sync::Arc;

const PUBLIC_URL: &str = "https://git.example.com";

fn rp() -> RelyingParty {
    RelyingParty::from_public_url(PUBLIC_URL).unwrap()
}

struct Instance {
    _dir: tempfile::TempDir,
    ctx: Arc<AppContext>,
    app: axum::Router,
}

impl Instance {
    /// `root` (an admin), `alice` and `bob`, all password users, with
    /// passkeys still off.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(ctx(dir.path()));
        user(&ctx, "root", true).await;
        user(&ctx, "alice", false).await;
        user(&ctx, "bob", false).await;
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        Instance { _dir: dir, ctx, app }
    }

    /// [`Instance::new`] with [`PUBLIC_URL`] already set.
    async fn with_passkeys() -> Self {
        let gq = Instance::new().await;
        let (mut settings, etag) = storage::settings::get_settings(gq.ctx.backend().as_ref(), &gq.ctx.cipher)
            .await
            .unwrap();
        settings.public_url = Some(PUBLIC_URL.to_string());
        storage::settings::put_settings(gq.ctx.backend().as_ref(), &settings, etag.as_deref(), &gq.ctx.cipher)
            .await
            .unwrap();
        gq
    }

    async fn stored_public_url(&self) -> Option<String> {
        storage::settings::get_settings(self.ctx.backend().as_ref(), &self.ctx.cipher)
            .await
            .unwrap()
            .0
            .public_url
    }

    async fn save_public_url(&self, admin: &Session, url: &str) -> Page {
        admin
            .post(&self.app, "/_settings/network", &[("public_url", url)])
            .await
    }

    async fn passkey_labels(&self, username: &str) -> Vec<String> {
        let owner = access::get_user(self.ctx.backend().as_ref(), &self.ctx.cipher, username)
            .await
            .unwrap()
            .0
            .unwrap();
        domain::passkey::list(&self.ctx, &owner.uid)
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.label)
            .collect()
    }

    fn sealed_challenge(&self, json: &Value) -> String {
        let sealed = self
            .ctx
            .cipher
            .seal(
                "passkey-challenge",
                json.to_string().as_bytes(),
                CompressionStrategy::None,
            )
            .unwrap();
        format!("passkey-challenge={}", B64.encode(sealed))
    }
}

/// A JSON POST as `passkey.js` sends it, carrying `cookies` as given.
async fn post_json(app: &axum::Router, path: &str, cookies: &str, body: &Value) -> Page {
    let mut builder = Request::post(path).header(header::CONTENT_TYPE, "application/json");
    if !cookies.is_empty() {
        builder = builder.header(header::COOKIE, cookies);
    }
    send(app, builder.body(Body::from(body.to_string())).unwrap()).await
}

fn set_cookie_line(page: &Page, name: &str) -> Option<String> {
    page.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(str::to_string)
}

fn clears_challenge(page: &Page) -> bool {
    set_cookie_line(page, "passkey-challenge").is_some_and(|line| line.starts_with("passkey-challenge=; "))
}

/// A registration begin: the options as JSON plus the challenge cookie.
async fn begin_registration(gq: &Instance, session: &Session) -> (Value, String) {
    let page = post_json(
        &gq.app,
        "/_account/passkeys/begin",
        &session.cookie,
        &json!({ "csrf": session.csrf, "reauth": PASSWORD }),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    (
        serde_json::from_str(&page.body).unwrap(),
        page.cookie("passkey-challenge"),
    )
}

async fn finish_registration(
    gq: &Instance,
    session: &Session,
    challenge_cookie: &str,
    challenge: &str,
    key: &Authenticator,
    label: &str,
) -> Page {
    let reg = key.register(challenge, UP | UV);
    post_json(
        &gq.app,
        "/_account/passkeys/finish",
        &format!("{}; {challenge_cookie}", session.cookie),
        &json!({
            "csrf": session.csrf,
            "label": label,
            "clientDataJSON": webauthn::encode_id(&reg.client_data_json),
            "attestationObject": webauthn::encode_id(&reg.attestation_object),
        }),
    )
    .await
}

async fn register(gq: &Instance, session: &Session, key: &Authenticator, label: &str) {
    let (options, cookie) = begin_registration(gq, session).await;
    let page = finish_registration(gq, session, &cookie, options["challenge"].as_str().unwrap(), key, label).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
}

fn assertion_body(key: &Authenticator, challenge: &str, flags: u8, counter: u32, csrf: &str, return_to: &str) -> Value {
    let parts = key.assert(challenge, flags, counter);
    json!({
        "csrf": csrf,
        "id": webauthn::encode_id(&key.credential_id),
        "clientDataJSON": webauthn::encode_id(&parts.client_data_json),
        "authenticatorData": webauthn::encode_id(&parts.authenticator_data),
        "signature": webauthn::encode_id(&parts.signature),
        "userHandle": null,
        "return_to": return_to,
    })
}

/// A passkey-only sign-in from a fresh `/_login` visit, as `passkey.js`
/// runs it: begin anchored on the page's `login-csrf`, then finish.
async fn passkey_login(gq: &Instance, key: &Authenticator, flags: u8, counter: u32, return_to: &str) -> Page {
    let form = Session::get_as(&gq.app, "/_login", None).await;
    assert!(form.body.contains(r#"data-passkey="sign-in""#), "{}", form.body);
    let anchor = form.cookie("login-csrf");
    let csrf = form.csrf();
    let begin = post_json(&gq.app, "/_login/passkey/begin", &anchor, &json!({ "csrf": csrf })).await;
    assert_eq!(begin.status, StatusCode::OK, "{}", begin.body);
    let options: Value = serde_json::from_str(&begin.body).unwrap();
    assert_eq!(options["rpId"], "git.example.com");
    assert_eq!(options["userVerification"], "required");
    assert_eq!(options["allowCredentials"], json!([]));
    let challenge = options["challenge"].as_str().unwrap();
    post_json(
        &gq.app,
        "/_login/passkey/finish",
        &format!("{anchor}; {}", begin.cookie("passkey-challenge")),
        &assertion_body(key, challenge, flags, counter, &csrf, return_to),
    )
    .await
}

async fn session_is_live(gq: &Instance, page: &Page) -> bool {
    let Some(line) = set_cookie_line(page, "web-session") else {
        return false;
    };
    let cookie = line.split(';').next().unwrap();
    Session::get_as(&gq.app, "/_account/security", Some(cookie))
        .await
        .status
        == StatusCode::OK
}

async fn enable_totp(ctx: &AppContext, username: &str) {
    let (user, etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap();
    let mut user = user.unwrap();
    user.totp_secret = Some(gitquarry_auth_crypto::totp::generate_secret());
    user.totp_enabled = true;
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_public_url_setting_accepts_https_domains_and_localhost_only() {
    let gq = Instance::new().await;
    let root = Session::login(&gq.app, "root").await;

    let page = root.get(&gq.app, "/_settings/network").await;
    assert!(page.body.contains(r#"name="public_url""#), "{}", page.body);

    for bad in [
        "http://git.example.com",
        "https://192.168.1.10",
        "https://[::1]",
        "ftp://git.example.com",
        "not a url",
    ] {
        let page = gq.save_public_url(&root, bad).await;
        assert!(page.body.contains("flash-err"), "{bad} accepted: {}", page.body);
        assert_eq!(gq.stored_public_url().await, None, "{bad} was stored");
    }

    for good in ["http://localhost:8080", PUBLIC_URL] {
        let page = gq.save_public_url(&root, good).await;
        assert!(page.body.contains("flash-ok"), "{good} refused: {}", page.body);
        assert_eq!(gq.stored_public_url().await.as_deref(), Some(good));
    }

    // Blank switches passkeys back off.
    gq.save_public_url(&root, "  ").await;
    assert_eq!(gq.stored_public_url().await, None);

    // Admin-only, like the rest of the page.
    let alice = Session::login(&gq.app, "alice").await;
    let page = gq.save_public_url(&alice, PUBLIC_URL).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert_eq!(gq.stored_public_url().await, None);
}

#[tokio::test]
async fn a_passkey_registered_from_the_security_page_is_listed_there() {
    let gq = Instance::with_passkeys().await;
    let alice = Session::login(&gq.app, "alice").await;

    let page = alice.get(&gq.app, "/_account/security").await;
    assert!(page.body.contains(r#"data-passkey="register""#), "{}", page.body);
    assert!(page.body.contains("/_assets/passkey.js?v="), "{}", page.body);

    let begin = post_json(
        &gq.app,
        "/_account/passkeys/begin",
        &alice.cookie,
        &json!({ "csrf": alice.csrf, "reauth": PASSWORD }),
    )
    .await;
    assert_eq!(begin.status, StatusCode::OK, "{}", begin.body);
    let line = set_cookie_line(&begin, "passkey-challenge").unwrap();
    assert!(line.ends_with("; HttpOnly; SameSite=Lax; Max-Age=300"), "{line}");
    let options: Value = serde_json::from_str(&begin.body).unwrap();
    let me = access::get_user(gq.ctx.backend().as_ref(), &gq.ctx.cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(options["rp"]["id"], "git.example.com");
    assert_eq!(options["user"]["id"], webauthn::encode_id(me.uid.as_str().as_bytes()));
    assert_eq!(options["user"]["name"], "alice");
    assert_eq!(options["authenticatorSelection"]["userVerification"], "required");
    assert_eq!(options["excludeCredentials"], json!([]));

    let key = Authenticator::new(Key::p256(1), b"alice-laptop", &rp());
    let done = finish_registration(
        &gq,
        &alice,
        &begin.cookie("passkey-challenge"),
        options["challenge"].as_str().unwrap(),
        &key,
        "<b>laptop</b>",
    )
    .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert!(clears_challenge(&done));
    assert!(done.body.starts_with(r#"<div id="passkeys-section">"#), "{}", done.body);
    assert_eq!(gq.passkey_labels("alice").await, ["<b>laptop</b>"]);

    let page = alice.get(&gq.app, "/_account/security").await;
    assert!(page.body.contains("&lt;b&gt;laptop&lt;/b&gt;"), "{}", page.body);
    assert!(!page.body.contains("<b>laptop</b>"));

    // A second begin tells the browser not to re-register the same key.
    let (options, _) = begin_registration(&gq, &alice).await;
    assert_eq!(
        options["excludeCredentials"],
        json!([{ "type": "public-key", "id": webauthn::encode_id(b"alice-laptop") }])
    );
}

#[tokio::test]
async fn registration_begin_needs_the_csrf_token_and_the_current_password() {
    let gq = Instance::with_passkeys().await;
    let alice = Session::login(&gq.app, "alice").await;

    for body in [
        json!({ "reauth": PASSWORD }),
        json!({ "csrf": "forged", "reauth": PASSWORD }),
        json!({ "csrf": alice.csrf, "reauth": "not the password" }),
        json!({ "csrf": alice.csrf }),
    ] {
        let page = post_json(&gq.app, "/_account/passkeys/begin", &alice.cookie, &body).await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{body}");
        assert!(set_cookie_line(&page, "passkey-challenge").is_none(), "{body}");
    }

    // Anonymous: sent to log in, nothing issued.
    let page = post_json(&gq.app, "/_account/passkeys/begin", "", &json!({ "csrf": "" })).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert!(set_cookie_line(&page, "passkey-challenge").is_none());
}

#[tokio::test]
async fn registration_finish_refuses_any_challenge_cookie_but_this_accounts_live_one() {
    let gq = Instance::with_passkeys().await;
    let alice = Session::login(&gq.app, "alice").await;
    let bob = Session::login(&gq.app, "bob").await;
    let key = Authenticator::new(Key::p256(1), b"alice-laptop", &rp());
    let challenge = webauthn::new_challenge();
    let far = now_secs() + 300;

    let bobs = begin_registration(&gq, &bob).await;
    let cases = [
        ("missing", String::new(), challenge.clone()),
        (
            "another purpose",
            gq.sealed_challenge(&json!({ "purpose": "login", "challenge": challenge, "username": null, "exp": far })),
            challenge.clone(),
        ),
        (
            "expired",
            gq.sealed_challenge(
                &json!({ "purpose": "register", "challenge": challenge, "username": "alice", "exp": 1 }),
            ),
            challenge.clone(),
        ),
        (
            "sealed as another cookie",
            {
                let sealed = gq
                    .ctx
                    .cipher
                    .seal(
                        "pending-totp",
                        json!({ "purpose": "register", "challenge": challenge, "username": "alice", "exp": far })
                            .to_string()
                            .as_bytes(),
                        CompressionStrategy::None,
                    )
                    .unwrap();
                format!("passkey-challenge={}", B64.encode(sealed))
            },
            challenge.clone(),
        ),
        (
            "another account's",
            bobs.1.clone(),
            bobs.0["challenge"].as_str().unwrap().to_string(),
        ),
    ];
    for (case, cookie, challenge) in cases {
        let page = finish_registration(&gq, &alice, &cookie, &challenge, &key, "laptop").await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{case}: {}", page.body);
        assert!(
            page.body.contains("no passkey operation in progress"),
            "{case}: {}",
            page.body
        );
        assert!(clears_challenge(&page), "{case}");
    }
    assert!(gq.passkey_labels("alice").await.is_empty());
    assert!(gq.passkey_labels("bob").await.is_empty());
}

#[tokio::test]
async fn a_challenge_cookie_is_spent_by_its_first_finish() {
    let gq = Instance::with_passkeys().await;
    let alice = Session::login(&gq.app, "alice").await;
    let (options, cookie) = begin_registration(&gq, &alice).await;
    let challenge = options["challenge"].as_str().unwrap();

    // A failed attempt spends it too: a ceremony is one try.
    let wrong_rp = Authenticator::new(
        Key::p256(1),
        b"alice-laptop",
        &RelyingParty::from_public_url("https://evil.example.org").unwrap(),
    );
    let page = finish_registration(&gq, &alice, &cookie, challenge, &wrong_rp, "laptop").await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST);

    let key = Authenticator::new(Key::p256(1), b"alice-laptop", &rp());
    let replay = finish_registration(&gq, &alice, &cookie, challenge, &key, "laptop").await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert!(
        replay.body.contains("no passkey operation in progress"),
        "{}",
        replay.body
    );
    assert!(gq.passkey_labels("alice").await.is_empty());
}

#[tokio::test]
async fn a_passkey_is_removed_only_by_its_owner_with_their_password() {
    let gq = Instance::with_passkeys().await;
    let alice = Session::login(&gq.app, "alice").await;
    let bob = Session::login(&gq.app, "bob").await;
    register(
        &gq,
        &alice,
        &Authenticator::new(Key::p256(1), b"alice-laptop", &rp()),
        "laptop",
    )
    .await;
    let owner = access::get_user(gq.ctx.backend().as_ref(), &gq.ctx.cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap();
    let key = domain::passkey::list(&gq.ctx, &owner.uid).await.unwrap().remove(0).key;

    let page = alice.get(&gq.app, "/_account/security").await;
    assert!(
        page.body.contains(&format!(r#"name="key" value="{key}""#)),
        "{}",
        page.body
    );

    let page = bob
        .post(
            &gq.app,
            "/_account/passkeys/remove",
            &[("key", &key), ("reauth", PASSWORD)],
        )
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(gq.passkey_labels("alice").await, ["laptop"]);

    for reauth in ["", "not the password"] {
        let page = alice
            .post(
                &gq.app,
                "/_account/passkeys/remove",
                &[("key", &key), ("reauth", reauth)],
            )
            .await;
        assert_eq!(page.status, StatusCode::SEE_OTHER, "{reauth:?}");
        assert_eq!(gq.passkey_labels("alice").await, ["laptop"], "{reauth:?}");
    }
    let page = post_form(
        &gq.app,
        "/_account/passkeys/remove",
        Some(&alice.cookie),
        &[("csrf", "forged"), ("key", &key), ("reauth", PASSWORD)],
    )
    .await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST);
    assert_eq!(gq.passkey_labels("alice").await, ["laptop"]);

    let page = alice
        .post(
            &gq.app,
            "/_account/passkeys/remove",
            &[("key", &key), ("reauth", PASSWORD)],
        )
        .await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.location(), "/_account/security");
    assert!(gq.passkey_labels("alice").await.is_empty());
}

#[tokio::test]
async fn a_passkey_alone_signs_its_owner_in_past_2fa_and_back_to_a_local_page() {
    let gq = Instance::with_passkeys().await;
    let alice = Session::login(&gq.app, "alice").await;
    let key = Authenticator::new(Key::p256(1), b"alice-laptop", &rp());
    register(&gq, &alice, &key, "laptop").await;
    // A 2FA account: the passkey counts as both factors.
    enable_totp(&gq.ctx, "alice").await;

    let page = passkey_login(&gq, &key, UP | UV, 1, "/team/proj").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(
        serde_json::from_str::<Value>(&page.body).unwrap(),
        json!({ "redirect": "/team/proj" })
    );
    let line = set_cookie_line(&page, "web-session").unwrap();
    assert!(line.contains("; HttpOnly; SameSite=Lax"), "{line}");
    assert!(set_cookie_line(&page, "pending-totp").is_none());
    assert!(clears_challenge(&page));
    assert!(session_is_live(&gq, &page).await);

    let page = passkey_login(&gq, &key, UP | UV, 2, "//evil.example.org/phish").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(
        serde_json::from_str::<Value>(&page.body).unwrap(),
        json!({ "redirect": "/" })
    );

    let owner = access::get_user(gq.ctx.backend().as_ref(), &gq.ctx.cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap();
    let stored = &domain::passkey::list(&gq.ctx, &owner.uid).await.unwrap()[0];
    assert_eq!(stored.sign_count, 2);
    assert!(stored.last_used_at.is_some());

    // A password change bumps the session epoch; a passkey session obeys it
    // like any other.
    let before = passkey_login(&gq, &key, UP | UV, 3, "/").await;
    assert!(session_is_live(&gq, &before).await);
    domain::user::set_password(
        &gq.ctx,
        "alice",
        Some("a brand new passphrase"),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(!session_is_live(&gq, &before).await);
}

#[tokio::test]
async fn a_passkey_sign_in_is_refused_without_uv_for_an_unknown_key_or_from_another_origin() {
    let gq = Instance::with_passkeys().await;
    let alice = Session::login(&gq.app, "alice").await;
    let key = Authenticator::new(Key::p256(1), b"alice-laptop", &rp());
    register(&gq, &alice, &key, "laptop").await;

    let unknown = Authenticator::new(Key::p256(2), b"never-registered", &rp());
    let mut phished = Authenticator::new(Key::p256(1), b"alice-laptop", &rp());
    phished.origin = "https://git.example.com.evil.example.org".to_string();
    for (case, key, flags) in [
        ("no user verification", &key, UP),
        ("unknown credential", &unknown, UP | UV),
        ("another origin", &phished, UP | UV),
    ] {
        let page = passkey_login(&gq, key, flags, 0, "/").await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{case}: {}", page.body);
        assert!(page.body.contains("that passkey didn"), "{case}: {}", page.body);
        assert!(set_cookie_line(&page, "web-session").is_none(), "{case}");
        assert!(clears_challenge(&page), "{case}");
    }

    // Begin is anchored on the login page's own token.
    let form = Session::get_as(&gq.app, "/_login", None).await;
    let page = post_json(
        &gq.app,
        "/_login/passkey/begin",
        &form.cookie("login-csrf"),
        &json!({ "csrf": "forged" }),
    )
    .await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST);
    assert!(set_cookie_line(&page, "passkey-challenge").is_none());
}

#[tokio::test]
async fn with_no_public_url_every_passkey_route_is_404_and_no_control_renders() {
    let gq = Instance::new().await;
    let alice = Session::login(&gq.app, "alice").await;

    let page = alice.get(&gq.app, "/_account/security").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.body.contains("data-passkey"), "{}", page.body);
    assert!(!page.body.contains("passkey.js"), "{}", page.body);
    assert!(page.body.contains("/_settings/network"), "no off hint: {}", page.body);

    let body = json!({ "csrf": alice.csrf, "reauth": PASSWORD });
    for path in ["/_account/passkeys/begin", "/_account/passkeys/finish"] {
        let page = post_json(&gq.app, path, &alice.cookie, &body).await;
        assert_eq!(page.status, StatusCode::NOT_FOUND, "{path}");
    }
    let page = alice
        .post(
            &gq.app,
            "/_account/passkeys/remove",
            &[("key", "0"), ("reauth", PASSWORD)],
        )
        .await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);

    let login = Session::get_as(&gq.app, "/_login", None).await;
    assert!(!login.body.contains("data-passkey"), "{}", login.body);
    let anchor = login.cookie("login-csrf");
    let body = json!({ "csrf": login.csrf() });
    for path in ["/_login/passkey/begin", "/_login/passkey/finish"] {
        let page = post_json(&gq.app, path, &anchor, &body).await;
        assert_eq!(page.status, StatusCode::NOT_FOUND, "{path}");
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
