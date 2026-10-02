//! Passkeys (WebAuthn) in the web UI: registering one from
//! `/_account/security`, removing it again, signing in with one alone from
//! `/_login`, and using one in place of the TOTP code at `/_login_totp`. The ceremonies themselves are
//! `style/passkey.js` calling `navigator.credentials`; this module hands it
//! the options, keeps the challenge in a sealed single-use cookie, and has
//! [`gitquarry_core::auth::domain::passkey`] check and store what comes back.
//!
//! Everything here hangs off [`relying_party`]: with no public URL set
//! (`ServerSettings::public_url`) passkeys are off, every route below
//! answers 404 and no page renders a passkey control. The relying party is
//! never derived from the request's own `Host` header, which would let any
//! name the server answers to mint credentials for itself.

use super::{login, reauth};
use crate::webui::{
    AppError, State_, effective_locale, field, get_cookie, require_csrf, require_login, set_cookie, settings_section,
};
use anyhow::anyhow;
use axum::body::Bytes;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use gitquarry_auth_crypto::webauthn::{self, RelyingParty};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::entities::User;
use gitquarry_core::auth::{access, domain};
use gitquarry_core::storage;
use gitquarry_i18n::Locale;
use maud::{Markup, PreEscaped, html};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::{LazyLock, Mutex, PoisonError};
use tracing::warn;

const CHALLENGE_COOKIE: &str = "passkey-challenge";
/// One browser prompt, same order as `login`'s pending-TOTP step.
const CHALLENGE_TTL_SECS: i64 = 300;
/// What the browser is told to give the user, matching the cookie's life.
const CEREMONY_TIMEOUT_MS: i64 = CHALLENGE_TTL_SECS * 1000;
const SECURITY_URL: &str = "/_account/security";
const SECTION_ID: &str = "passkeys-section";

/// This instance as a relying party, or `None` while passkeys are off. The
/// stored URL was validated when an admin saved it; one that no longer
/// parses (edited in the bucket by hand) turns passkeys off rather than
/// letting anything guess.
pub(super) async fn relying_party(ctx: &AppContext) -> Option<RelyingParty> {
    let settings = match storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await {
        Ok((settings, _)) => settings,
        Err(e) => {
            warn!(target: "webui::passkey", error = %e, "couldn't read settings; passkeys off for this request");
            return None;
        }
    };
    let url = settings.public_url?;
    match RelyingParty::from_public_url(&url) {
        Ok(rp) => Some(rp),
        Err(e) => {
            warn!(target: "webui::passkey", public_url = %url, error = %e, "stored public URL is unusable; passkeys off");
            None
        }
    }
}

async fn require_rp(ctx: &AppContext, locale: Locale) -> Result<RelyingParty, AppError> {
    relying_party(ctx)
        .await
        .ok_or_else(|| AppError::not_found(anyhow!("{}", gitquarry_i18n::shared::auth::passkeys_off(locale))))
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum Purpose {
    Register,
    Login,
    SecondFactor,
}

/// What a begin step leaves for its finish step. Sealed under
/// [`CHALLENGE_COOKIE`] as AAD, so a sealed value from any other cookie
/// can't be passed off as one.
#[derive(Serialize, Deserialize)]
struct ChallengeCookie {
    purpose: Purpose,
    challenge: String,
    /// Who the ceremony is for; `None` for a sign-in, where the passkey
    /// itself says who.
    username: Option<String>,
    /// The real deadline — see `login::FlowCookie::exp` for why `Max-Age`
    /// alone isn't one.
    exp: u64,
}

fn plant_challenge(
    ctx: &AppContext,
    headers: &HeaderMap,
    purpose: Purpose,
    username: Option<&str>,
) -> Result<(String, (HeaderName, String)), AppError> {
    let cookie = ChallengeCookie {
        purpose,
        challenge: webauthn::new_challenge(),
        username: username.map(str::to_string),
        exp: login::deadline(CHALLENGE_TTL_SECS),
    };
    let sealed = ctx.cipher.seal(
        CHALLENGE_COOKIE,
        &serde_json::to_vec(&cookie)?,
        gitquarry_core::crypto::CompressionStrategy::None,
    )?;
    let set = set_cookie(headers, CHALLENGE_COOKIE, &B64.encode(sealed), CHALLENGE_TTL_SECS);
    Ok((cookie.challenge, set))
}

/// Challenges a finish step already consumed. The browser is told to drop
/// the cookie (see [`clear_challenge`]), but only a cooperating one does;
/// without this a captured cookie and response could be replayed until
/// `exp`, and a passkey that doesn't count signatures (most synced ones
/// report 0 every time) gives the counter check nothing to catch. Per
/// process: another instance behind the same bucket keeps its own. Bounded
/// so a flood of ceremonies can't grow it — an entry pushed out early only
/// loses this extra check, never the challenge match itself.
static SPENT_CHALLENGES: LazyLock<Mutex<lru::LruCache<String, ()>>> = LazyLock::new(|| {
    Mutex::new(lru::LruCache::new(
        NonZeroUsize::new(4096).expect("a non-zero capacity"),
    ))
});

/// `true` the first time `challenge` is seen. A `Mutex` because the check
/// and the insert have to be one step, and the LRU reorders even on reads.
fn first_use(challenge: &str) -> bool {
    SPENT_CHALLENGES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .put(challenge.to_string(), ())
        .is_none()
}

/// The challenge a finish step must be answering, if the cookie is live,
/// unspent, of this `purpose` and for this `username`. Any other outcome is
/// the same refusal: none of them is something the person can fix but by
/// starting over.
fn take_challenge(
    ctx: &AppContext,
    headers: &HeaderMap,
    purpose: Purpose,
    username: Option<&str>,
    locale: Locale,
) -> Result<String, AppError> {
    let refused = || {
        AppError::bad_request(anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::no_passkey_ceremony_in_progress(locale)
        ))
    };
    let raw = get_cookie(headers, CHALLENGE_COOKIE).ok_or_else(refused)?;
    let sealed = B64.decode(raw).map_err(|_| refused())?;
    let plain = ctx.cipher.open(CHALLENGE_COOKIE, &sealed).map_err(|_| refused())?;
    let cookie: ChallengeCookie = serde_json::from_slice(&plain).map_err(|_| refused())?;
    if cookie.purpose != purpose || cookie.username.as_deref() != username || login::expired(cookie.exp) {
        return Err(refused());
    }
    if !first_use(&cookie.challenge) {
        return Err(refused());
    }
    Ok(cookie.challenge)
}

/// Every finish step's response, success or not, drops the challenge
/// cookie: a ceremony is one attempt, and a retry starts from begin.
fn clear_challenge(headers: &HeaderMap, outcome: Result<Response, AppError>) -> Response {
    let mut response = outcome.unwrap_or_else(IntoResponse::into_response);
    let (name, value) = set_cookie(headers, CHALLENGE_COOKIE, "", 0);
    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().append(name, value);
    }
    response
}

fn with_cookie(
    mut response: Response,
    (name, value): (HeaderName, String),
    locale: Locale,
) -> Result<Response, AppError> {
    let value = HeaderValue::from_str(&value)
        .map_err(|_| AppError::from(anyhow!("{}", gitquarry_i18n::shared::auth::building_set_cookie(locale))))?;
    response.headers_mut().append(name, value);
    Ok(response)
}

/// Bodies are parsed here rather than by axum's `Json` extractor so that a
/// request with passkeys off is a 404 whatever it carries, not a 415/422
/// that says the route is live.
fn parse_json<T: DeserializeOwned>(body: &[u8], locale: Locale) -> Result<T, AppError> {
    serde_json::from_slice(body).map_err(|_| malformed(locale))
}

fn malformed(locale: Locale) -> AppError {
    AppError::bad_request(anyhow!(
        "{}",
        gitquarry_i18n::shared::auth::malformed_passkey_request(locale)
    ))
}

fn decode(value: &str, locale: Locale) -> Result<Vec<u8>, AppError> {
    webauthn::decode_b64url(value).map_err(|_| malformed(locale))
}

/// A stored passkey's key, as a remove form sends it back: `passkey_key`'s
/// SHA-256 hex. Checked before it becomes an S3 key segment.
fn is_passkey_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Deserialize)]
pub(crate) struct RegisterBegin {
    #[serde(default)]
    csrf: String,
    /// Current password or 2FA code — a passkey signs in on its own, so a
    /// stolen session must not be able to plant one (same gate as adding
    /// an SSH key).
    #[serde(default)]
    reauth: String,
    /// Which of the form's two buttons was pressed. Absent leaves the
    /// browser's own chooser alone.
    #[serde(default)]
    kind: Option<AuthenticatorKind>,
}

/// Where the new passkey should live. Both end up the same discoverable,
/// user-verified credential; the choice only steers which prompt the
/// browser opens first, since Chrome otherwise goes straight to its own
/// password manager and hides the security-key path behind "use another
/// device".
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum AuthenticatorKind {
    /// A roaming FIDO2 key (USB/NFC/BLE). It must hold resident keys,
    /// since `/_login` signs in from the credential alone.
    SecurityKey,
    /// This device's platform authenticator, a password manager or wallet
    /// (iCloud Keychain, Google Password Manager, Bitwarden…), or a phone
    /// over hybrid. No `authenticatorAttachment` here: a phone over hybrid
    /// and some wallet extensions count as cross-platform.
    Device,
}

impl AuthenticatorKind {
    /// WebAuthn L3 `hints`, the attachment that goes with them.
    fn selection(self) -> (serde_json::Value, Option<&'static str>) {
        match self {
            Self::SecurityKey => (serde_json::json!(["security-key"]), Some("cross-platform")),
            Self::Device => (serde_json::json!(["client-device", "hybrid"]), None),
        }
    }
}

/// `POST /_account/passkeys/begin`: `PublicKeyCredentialCreationOptions`
/// for the logged-in account, binary values base64url for `passkey.js` to
/// convert.
pub(crate) async fn handler_register_begin(
    State(ctx): State_,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let rp = require_rp(&ctx, locale).await?;
    let me = match require_login(&ctx, &headers, SECURITY_URL).await {
        Ok(user) => user,
        Err(resp) => return Ok(*resp),
    };
    let form: RegisterBegin = parse_json(&body, locale)?;
    require_csrf(&headers, &form.csrf)?;
    reauth::require_reauth_for_credential_change(&ctx, &me, form.reauth.trim(), locale)
        .await
        .map_err(|e| AppError::bad_request(anyhow!(e)))?;

    let existing = domain::passkey::list(&ctx, &me.uid).await?;
    let (challenge, cookie) = plant_challenge(&ctx, &headers, Purpose::Register, Some(&me.username))?;
    let mut selection = serde_json::json!({ "residentKey": "required", "userVerification": "required" });
    let mut options = serde_json::json!({
        "challenge": challenge,
        "rp": { "id": rp.id, "name": "GitQuarry" },
        "user": {
            // What a discoverable credential hands back as its user handle
            // at sign-in, and what `domain::passkey::verify` compares with.
            "id": webauthn::encode_id(me.uid.as_str().as_bytes()),
            "name": me.username,
            "displayName": me.username,
        },
        "pubKeyCredParams": [
            { "type": "public-key", "alg": webauthn::ES256 },
            { "type": "public-key", "alg": webauthn::EDDSA },
        ],
        "excludeCredentials": existing
            .iter()
            .map(|p| serde_json::json!({ "type": "public-key", "id": p.credential_id }))
            .collect::<Vec<_>>(),
        "attestation": "none",
        "timeout": CEREMONY_TIMEOUT_MS,
    });
    if let Some((hints, attachment)) = form.kind.map(AuthenticatorKind::selection) {
        options["hints"] = hints;
        if let Some(attachment) = attachment {
            selection["authenticatorAttachment"] = attachment.into();
        }
    }
    options["authenticatorSelection"] = selection;
    with_cookie(axum::Json(options).into_response(), cookie, locale)
}

#[derive(Deserialize)]
pub(crate) struct RegisterFinish {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    label: String,
    #[serde(default, rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(default, rename = "attestationObject")]
    attestation_object: String,
}

/// `POST /_account/passkeys/finish`: checks the browser's
/// `navigator.credentials.create()` response against the challenge begin
/// issued to this same account, stores the passkey, and answers with the
/// refreshed passkeys section for `passkey.js` to swap in.
pub(crate) async fn handler_register_finish(State(ctx): State_, headers: HeaderMap, body: Bytes) -> Response {
    let outcome = register_finish(&ctx, &headers, &body).await;
    clear_challenge(&headers, outcome)
}

async fn register_finish(ctx: &AppContext, headers: &HeaderMap, body: &[u8]) -> Result<Response, AppError> {
    let locale = effective_locale(ctx, headers).await;
    let rp = require_rp(ctx, locale).await?;
    let me = match require_login(ctx, headers, SECURITY_URL).await {
        Ok(user) => user,
        Err(resp) => return Ok(*resp),
    };
    let form: RegisterFinish = parse_json(body, locale)?;
    require_csrf(headers, &form.csrf)?;
    let challenge = take_challenge(ctx, headers, Purpose::Register, Some(&me.username), locale)?;
    let label = form.label.trim();
    if access::validate_passkey_label(label).is_err() {
        return Err(AppError::bad_request(anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::invalid_passkey_label(locale)
        )));
    }
    let credential = webauthn::verify_registration(
        &rp,
        &challenge,
        &decode(&form.client_data_json, locale)?,
        &decode(&form.attestation_object, locale)?,
        true,
    )
    .map_err(|e| {
        warn!(target: "webui::passkey", user = %me.username, error = %e, "passkey registration refused");
        AppError::bad_request(anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::passkey_not_registered(locale)
        ))
    })?;
    // Its refusals (the per-account cap, an id already registered) are
    // meant for the person, hence a 400 rather than a server error.
    let passkey = domain::passkey::register(ctx, &me, credential, label)
        .await
        .map_err(AppError::bad_request)?;
    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "passkey.register",
        &me.username,
        Some(passkey.label),
    )
    .await;
    let csrf = crate::webui::csrf_field(headers);
    Ok(Html(render_section(ctx, &me, &csrf, locale).await?.into_string()).into_response())
}

#[derive(Deserialize)]
pub(crate) struct RemoveForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    reauth: String,
}

/// `POST /_account/passkeys/remove`: a plain form (async from the section's
/// own remove buttons), gated on the current password like every other
/// credential change on this page.
pub(crate) async fn handler_remove(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<RemoveForm>,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    require_rp(&ctx, locale).await?;
    let me = match require_login(&ctx, &headers, SECURITY_URL).await {
        Ok(user) => user,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let is_async = headers.contains_key(crate::webui::ASYNC_HEADER);
    // An async submit shows the reason as a toast; a plain one goes back to
    // the page it came from, which is the only URL the browser can reload.
    let fail = |message: String| {
        if is_async {
            AppError::bad_request(anyhow!(message))
        } else {
            AppError::flash_back(&headers, SECURITY_URL, anyhow!(message))
        }
    };
    // Skipped only for an account with neither a password nor a 2FA
    // secret, which has nothing to re-prove — the same exception adding a
    // passkey makes, so such an account can remove what it added.
    reauth::require_reauth_for_credential_change(&ctx, &me, form.reauth.trim(), locale)
        .await
        .map_err(fail)?;
    let not_found = || fail(gitquarry_i18n::shared::auth::passkey_not_found(locale));
    if !is_passkey_key(&form.key) {
        return Err(not_found());
    }
    if !domain::passkey::remove(&ctx, &me.uid, &form.key).await? {
        return Err(not_found());
    }
    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "passkey.remove",
        &me.username,
        Some(form.key),
    )
    .await;
    if is_async {
        let csrf = crate::webui::csrf_field(&headers);
        return Ok(Html(render_section(&ctx, &me, &csrf, locale).await?.into_string()).into_response());
    }
    Ok(Redirect::to(SECURITY_URL).into_response())
}

/// The passkeys block of `/_account/security`, wrapped in the one `id`
/// both the register flow and the remove buttons replace. `csrf` is the
/// rendered hidden field, as every form on that page takes it.
pub(super) async fn render_section(
    ctx: &AppContext,
    user: &User,
    csrf: &str,
    locale: Locale,
) -> Result<Markup, AppError> {
    use gitquarry_i18n::web::auth::security as t;
    let enabled = relying_party(ctx).await.is_some();
    let passkeys = if enabled {
        domain::passkey::list(ctx, &user.uid).await?
    } else {
        Vec::new()
    };
    // Same rule as the rest of the page: only asked for where there is
    // something to re-prove.
    let needs_reauth = user.password_hash.is_some() || user.totp_secret.is_some();
    Ok(html! {
        div id=(SECTION_ID) {
            (settings_section(&t::passkeys_heading(locale), Some(&t::passkeys_purpose(locale)), html! {
                @if !enabled {
                    p class="muted" { (t::passkeys_off_hint(locale)) }
                } @else {
                    table class="list" {
                        tr {
                            th { (t::passkey_name_column_header(locale)) }
                            th { (t::passkey_created_column_header(locale)) }
                            th { (t::passkey_last_used_column_header(locale)) }
                            th {}
                        }
                        @if passkeys.is_empty() {
                            tr { td colspan="4" { em { (t::no_passkeys_yet_hint(locale)) } } }
                        }
                        @for passkey in &passkeys {
                            tr {
                                td {
                                    (passkey.label)
                                    @if passkey.backup_eligible {
                                        " " span class="badge" { (t::passkey_synced_badge(locale)) }
                                    }
                                }
                                td { (passkey.created_at) }
                                td { (passkey.last_used_at.as_deref().map_or_else(|| t::passkey_never_used(locale), str::to_string)) }
                                td {
                                    form method="post" action="/_account/passkeys/remove" class="inline-form" data-async data-async-target=(format!("#{SECTION_ID}")) data-confirm=(t::remove_passkey_confirm(locale, &passkey.label)) {
                                        (PreEscaped(csrf))
                                        input type="hidden" name="key" value=(passkey.key);
                                        @if needs_reauth {
                                            input type="password" name="reauth" size="16" autocomplete="current-password"
                                                placeholder=(t::reauth_placeholder(locale)) aria-label=(t::reauth_label(locale));
                                        }
                                        button type="submit" { (t::delete_button(locale)) }
                                    }
                                }
                            }
                        }
                    }
                    // Hidden until `passkey.js` finds WebAuthn in this
                    // browser; without it (or without JS) there is nothing
                    // this form could do.
                    form data-passkey="register" data-begin-url="/_account/passkeys/begin" data-finish-url="/_account/passkeys/finish" data-target=(format!("#{SECTION_ID}")) hidden {
                        (PreEscaped(csrf))
                        (field(&t::passkey_name_label(locale), None, html! {
                            input name="label" size="30" maxlength="64" required placeholder=(t::passkey_name_placeholder(locale));
                        }))
                        @if needs_reauth {
                            (field(&t::reauth_label(locale), Some(&t::reauth_hint(locale)), html! {
                                input type="password" name="reauth" size="30" autocomplete="current-password";
                            }))
                        }
                        div class="field-error" data-passkey-flash hidden {}
                        button type="submit" name="kind" value="security-key" { (t::add_security_key_button(locale)) }
                        " "
                        button type="submit" name="kind" value="device" { (t::add_device_passkey_button(locale)) }
                        p class="muted" { (t::add_passkey_kind_hint(locale)) }
                    }
                    script src=(crate::webui::passkey_js_url()) defer {}
                }
            }))
        }
    })
}

/// The relying party a sign-in may use: passkeys on *and* login enforced —
/// on an instance where nobody has to log in (no OIDC, no password yet) a
/// passkey session would be the one way past a gate that is meant to be
/// held open by the bootstrap grant instead.
pub(super) async fn sign_in_relying_party(ctx: &AppContext) -> Option<RelyingParty> {
    if !login::login_enforced(ctx).await {
        return None;
    }
    relying_party(ctx).await
}

async fn require_sign_in_rp(ctx: &AppContext, locale: Locale) -> Result<RelyingParty, AppError> {
    sign_in_relying_party(ctx)
        .await
        .ok_or_else(|| AppError::not_found(anyhow!("{}", gitquarry_i18n::shared::auth::passkeys_off(locale))))
}

/// The "sign in with a passkey" button for a page whose own CSRF anchor
/// (`csrf`, already derived) the begin and finish steps under `base` check.
pub(super) fn sign_in_control(label: &str, base: &str, csrf: &str, return_to: &str) -> Markup {
    html! {
        div data-passkey="sign-in" data-begin-url=(format!("{base}/begin")) data-finish-url=(format!("{base}/finish"))
            data-csrf=(csrf) data-return-to=(return_to) hidden {
            p { button type="button" data-passkey-start { (label) } }
            div class="field-error" data-passkey-flash hidden {}
        }
        script src=(crate::webui::passkey_js_url()) defer {}
    }
}

#[derive(Deserialize)]
pub(crate) struct CsrfOnly {
    #[serde(default)]
    csrf: String,
}

/// `POST /_login/passkey/begin`: `PublicKeyCredentialRequestOptions` for a
/// discoverable credential — no `allowCredentials`, the authenticator
/// offers whichever passkeys it holds for this site. Anchored on the same
/// `login-csrf` cookie as the password form: logging someone into an
/// account of the attacker's choosing is the attack either way.
pub(crate) async fn handler_login_begin(
    State(ctx): State_,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let rp = require_sign_in_rp(&ctx, locale).await?;
    let form: CsrfOnly = parse_json(&body, locale)?;
    crate::webui::require_csrf_for(&headers, login::LOGIN_CSRF_COOKIE, &form.csrf)?;
    let (challenge, cookie) = plant_challenge(&ctx, &headers, Purpose::Login, None)?;
    let options = serde_json::json!({
        "challenge": challenge,
        "rpId": rp.id,
        "userVerification": "required",
        "allowCredentials": [],
        "timeout": CEREMONY_TIMEOUT_MS,
    });
    with_cookie(axum::Json(options).into_response(), cookie, locale)
}

/// A `navigator.credentials.get()` response as `passkey.js` posts it.
#[derive(Deserialize)]
pub(crate) struct AssertionForm {
    #[serde(default)]
    csrf: String,
    /// `rawId`, base64url.
    #[serde(default)]
    id: String,
    #[serde(default, rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(default, rename = "authenticatorData")]
    authenticator_data: String,
    #[serde(default)]
    signature: String,
    #[serde(default, rename = "userHandle")]
    user_handle: Option<String>,
    #[serde(default)]
    return_to: String,
}

impl AssertionForm {
    /// Decoded for `domain::passkey::verify`. The id is re-encoded rather
    /// than passed through, so only canonical base64url reaches the lookup.
    fn assertion(&self, locale: Locale) -> Result<domain::passkey::Assertion, AppError> {
        Ok(domain::passkey::Assertion {
            credential_id: webauthn::encode_id(&decode(&self.id, locale)?),
            client_data_json: decode(&self.client_data_json, locale)?,
            authenticator_data: decode(&self.authenticator_data, locale)?,
            signature: decode(&self.signature, locale)?,
            user_handle: match self.user_handle.as_deref() {
                Some(handle) if !handle.is_empty() => Some(decode(handle, locale)?),
                _ => None,
            },
        })
    }
}

fn did_not_work(locale: Locale) -> AppError {
    AppError::bad_request(anyhow!(
        "{}",
        gitquarry_i18n::shared::auth::passkey_did_not_work(locale)
    ))
}

/// Where `passkey.js` goes once the session cookie is set.
fn redirect_json(to: &str) -> Response {
    axum::Json(serde_json::json!({ "redirect": to })).into_response()
}

/// `POST /_login/passkey/finish`: a passkey alone signs its owner in. It
/// counts as both factors — something held plus the PIN or biometric the
/// authenticator verified (`require_uv`) — so a 2FA account gets no TOTP
/// step after it.
pub(crate) async fn handler_login_finish(State(ctx): State_, headers: HeaderMap, body: Bytes) -> Response {
    let outcome = login_finish(&ctx, &headers, &body).await;
    clear_challenge(&headers, outcome)
}

async fn login_finish(ctx: &AppContext, headers: &HeaderMap, body: &[u8]) -> Result<Response, AppError> {
    let locale = effective_locale(ctx, headers).await;
    let rp = require_sign_in_rp(ctx, locale).await?;
    let form: AssertionForm = parse_json(body, locale)?;
    crate::webui::require_csrf_for(headers, login::LOGIN_CSRF_COOKIE, &form.csrf)?;
    let challenge = take_challenge(ctx, headers, Purpose::Login, None, locale)?;
    let assertion = form.assertion(locale)?;
    let (user, passkey) = match domain::passkey::verify(ctx, &rp, &challenge, &assertion, None, true).await? {
        Ok(verified) => verified,
        Err(refusal) => {
            warn!(target: "webui::passkey", credential = %assertion.credential_id, refusal = %refusal, "passkey sign-in refused");
            return Err(did_not_work(locale));
        }
    };
    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &user.username,
        "passkey.login",
        &user.username,
        Some(passkey.label),
    )
    .await;
    let return_to = login::safe_return_to(&form.return_to);
    login::issue_session(headers, &ctx.cipher, &user, &[], redirect_json(return_to), locale)
}

/// The account a pending-TOTP login is for, and its passkeys, when the
/// second-factor passkey button is worth showing at all.
async fn second_factor_passkeys(
    ctx: &AppContext,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<Option<(User, Vec<gitquarry_core::auth::entities::Passkey>)>, AppError> {
    let pending = login::open_pending_totp(ctx, headers, locale)?;
    let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &pending.username)
        .await?
        .0
    else {
        return Ok(None);
    };
    let passkeys = domain::passkey::list(ctx, &user.uid).await?;
    Ok((!passkeys.is_empty()).then_some((user, passkeys)))
}

/// Whether `/_login_totp` offers "use a passkey instead": passkeys on and
/// the pending account has at least one.
pub(super) async fn second_factor_offered(ctx: &AppContext, headers: &HeaderMap, locale: Locale) -> bool {
    relying_party(ctx).await.is_some() && matches!(second_factor_passkeys(ctx, headers, locale).await, Ok(Some(_)))
}

/// `POST /_login_totp/passkey/begin`: request options naming the pending
/// account's own passkeys. Anchored on the `pending-totp` cookie, which is
/// also what says whose they are. `userVerification` is only preferred:
/// the password already stood in for "something known".
pub(crate) async fn handler_second_factor_begin(
    State(ctx): State_,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let rp = require_rp(&ctx, locale).await?;
    let form: CsrfOnly = parse_json(&body, locale)?;
    crate::webui::require_csrf_for(&headers, login::PENDING_TOTP_COOKIE, &form.csrf)?;
    let Some((user, passkeys)) = second_factor_passkeys(&ctx, &headers, locale).await? else {
        return Err(did_not_work(locale));
    };
    let (challenge, cookie) = plant_challenge(&ctx, &headers, Purpose::SecondFactor, Some(&user.username))?;
    let options = serde_json::json!({
        "challenge": challenge,
        "rpId": rp.id,
        "userVerification": "preferred",
        "allowCredentials": passkeys
            .iter()
            .map(|p| serde_json::json!({ "type": "public-key", "id": p.credential_id }))
            .collect::<Vec<_>>(),
        "timeout": CEREMONY_TIMEOUT_MS,
    });
    with_cookie(axum::Json(options).into_response(), cookie, locale)
}

/// `POST /_login_totp/passkey/finish`: completes the pending login exactly
/// as a correct code would, and a refusal counts against the same
/// `LoginAttempts` bucket a wrong code does — guesses at the second factor
/// add up whichever way they're made.
pub(crate) async fn handler_second_factor_finish(State(ctx): State_, headers: HeaderMap, body: Bytes) -> Response {
    let outcome = second_factor_finish(&ctx, &headers, &body).await;
    clear_challenge(&headers, outcome)
}

async fn second_factor_finish(ctx: &AppContext, headers: &HeaderMap, body: &[u8]) -> Result<Response, AppError> {
    let locale = effective_locale(ctx, headers).await;
    let rp = require_rp(ctx, locale).await?;
    let form: AssertionForm = parse_json(body, locale)?;
    crate::webui::require_csrf_for(headers, login::PENDING_TOTP_COOKIE, &form.csrf)?;
    let pending = login::open_pending_totp(ctx, headers, locale)?;
    let challenge = take_challenge(ctx, headers, Purpose::SecondFactor, Some(&pending.username), locale)?;
    let assertion = form.assertion(locale)?;

    let penalty = ctx.login_attempts.penalty(&pending.username);
    if !penalty.is_zero() {
        tokio::time::sleep(penalty).await;
    }
    let Some(expected) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &pending.username)
        .await?
        .0
    else {
        ctx.login_attempts.record_failure(&pending.username);
        return Err(did_not_work(locale));
    };
    let (user, passkey) = match domain::passkey::verify(ctx, &rp, &challenge, &assertion, Some(&expected.uid), false)
        .await?
    {
        Ok(verified) => verified,
        Err(refusal) => {
            ctx.login_attempts.record_failure(&pending.username);
            warn!(target: "webui::passkey", user = %pending.username, credential = %assertion.credential_id, refusal = %refusal, "passkey second factor refused");
            return Err(did_not_work(locale));
        }
    };
    ctx.login_attempts.clear(&user.username);
    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &user.username,
        "passkey.second_factor",
        &user.username,
        Some(passkey.label),
    )
    .await;
    login::issue_session(
        headers,
        &ctx.cipher,
        &user,
        &[login::PENDING_TOTP_COOKIE],
        redirect_json(&pending.return_to),
        locale,
    )
}
