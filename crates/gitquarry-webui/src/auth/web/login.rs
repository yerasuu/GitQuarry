//! Login for the web UI, two ways in, both resolving to the same
//! [`gitquarry_core::auth::entities::User`]:
//! - OIDC: the `/_login_oidc` -> IdP -> `/_callback` round trip, resolved
//!   against `access.json` by verified email.
//! - password: a plain username/password form, POSTed to `/_login`,
//!   resolved against `access.json` by username + [`User::password_hash`].
//!   Any user can have a password, not just admins — `User::is_admin` is
//!   what actually grants the admin pages, same as it would via OIDC.
//!
//! `/_login` (GET) is the shared landing page: it shows the password form
//! only if some user actually has one set (else there'd be nothing to log
//! into), plus an OIDC link if that's configured too, so callers that
//! redirect there (the admin-page gate, the private-repo gate) don't need to
//! know which method is in use. Either resolves to the same [`User`] on
//! every request that cares (admin pages, `private` repos) via
//! [`current_user`].
//!
//! No server-side session store: every cookie this module sets is sealed
//! with the process's own [`gitquarry_core::crypto::Cipher`] (same one that encrypts
//! everything uploaded to the bucket, different AAD namespace — see
//! [`gitquarry_core::app::AppContext::cipher`]). Nothing about a session survives
//! only in memory; a restart doesn't log anyone out, and there's nothing to
//! garbage-collect.

use crate::webui::{
    AppError, State_, effective_density, effective_locale, effective_theme, esc, get_cookie, is_local_redirect, layout,
    set_cookie, url_encode,
};
use anyhow::{Context, Result};
use axum::extract::{Form, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use gitquarry_auth_crypto::password;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::entities::User;
use maud::html;
use openidconnect::core::CoreAuthenticationFlow;
use openidconnect::{AuthorizationCode, CsrfToken, Nonce, PkceCodeChallenge, PkceCodeVerifier, Scope, TokenResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FLOW_COOKIE: &str = "oidc-flow";
const FLOW_TTL_SECS: i64 = 600;
pub(crate) const SESSION_COOKIE: &str = "web-session";
const SESSION_TTL: Duration = Duration::from_secs(7 * 86400);
const PENDING_TOTP_COOKIE: &str = "pending-totp";
/// Anchors the CSRF token on `/_login`'s own form. Everything else in this
/// app double-submits against the session cookie, which by definition
/// doesn't exist yet here — see [`crate::webui::require_csrf_for`].
pub(crate) const LOGIN_CSRF_COOKIE: &str = "login-csrf";
/// Same order as [`FLOW_TTL_SECS`]: long enough to fill in a form, short
/// enough that a stale one isn't worth keeping.
const LOGIN_CSRF_TTL_SECS: i64 = 600;
/// Shorter than [`FLOW_TTL_SECS`]: this step is one code entry, not an IdP
/// round trip.
const PENDING_TOTP_TTL_SECS: i64 = 300;

/// Re-exported so this module's own call sites (and its tests) keep naming
/// it locally — the value itself is shared with git-smart-HTTP's Basic auth,
/// see [`password::DUMMY_HASH`].
pub use gitquarry_auth_crypto::password::DUMMY_HASH;

/// What survives the redirect to the IdP and back: the CSRF/nonce/PKCE
/// values `openidconnect` generated for this attempt, plus where to send
/// the browser once login succeeds.
///
/// `exp` is the real deadline. The `Max-Age` on the cookie only tells a
/// *cooperating* browser when to drop it — a client that keeps sending the
/// sealed blob past that point would otherwise get an indefinitely
/// replayable flow, since nothing server-side was checking. Same reason
/// [`SessionCookie`] carries its own `exp` rather than trusting `Max-Age`.
#[cfg(any(test, feature = "test-util"))]
#[derive(Serialize, Deserialize)]
pub struct FlowCookie {
    pub csrf: String,
    pub nonce: String,
    pub pkce_verifier: String,
    pub return_to: String,
    #[serde(default)]
    pub exp: u64,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Serialize, Deserialize)]
struct FlowCookie {
    csrf: String,
    nonce: String,
    pkce_verifier: String,
    return_to: String,
    #[serde(default)]
    exp: u64,
}

/// Who a session cookie names, before it's resolved against `access.json`
/// (re-read on every request — see [`current_user`], not cached here).
#[cfg(any(test, feature = "test-util"))]
#[derive(Serialize, Deserialize)]
pub enum SessionSubject {
    Email(String),
    Username(String),
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Serialize, Deserialize)]
enum SessionSubject {
    Email(String),
    Username(String),
}

#[cfg(any(test, feature = "test-util"))]
#[derive(Serialize, Deserialize)]
pub struct SessionCookie {
    pub subject: SessionSubject,
    pub exp: u64,
    /// The subject's `User::session_epoch` when this cookie was issued —
    /// see that field for why. `#[serde(default)]` so a cookie sealed
    /// before this field existed reads as `0` and keeps working against a
    /// stored record that is also still `0`.
    #[serde(default)]
    pub epoch: u64,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Serialize, Deserialize)]
struct SessionCookie {
    subject: SessionSubject,
    exp: u64,
    /// The subject's `User::session_epoch` when this cookie was issued —
    /// see that field for why. `#[serde(default)]` so a cookie sealed
    /// before this field existed reads as `0` and keeps working against a
    /// stored record that is also still `0`.
    #[serde(default)]
    epoch: u64,
}

/// What survives the redirect from `/_login` to `/_login_totp` and back: the
/// already-verified username (password already matched, this cookie is only
/// proof of *that*, never trusted as proof the code will also match — see
/// [`handler_login_totp_post`], which re-fetches the user fresh) plus where
/// to send the browser once the code verifies too.
#[cfg(any(test, feature = "test-util"))]
#[derive(Serialize, Deserialize)]
pub struct PendingTotpCookie {
    pub username: String,
    pub return_to: String,
    /// The real deadline on the "password already matched" claim — see
    /// [`FlowCookie::exp`] for why `Max-Age` alone isn't one.
    #[serde(default)]
    pub exp: u64,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Serialize, Deserialize)]
struct PendingTotpCookie {
    username: String,
    return_to: String,
    /// The real deadline on the "password already matched" claim — see
    /// [`FlowCookie::exp`] for why `Max-Age` alone isn't one.
    #[serde(default)]
    exp: u64,
}

/// Rejects anything that isn't an in-app path, so a crafted `?return_to=`
/// can't turn our own `/_login` into an open redirect to an attacker's site.
/// See [`is_local_redirect`] for what counts as safe.
#[cfg(any(test, feature = "test-util"))]
pub fn safe_return_to(s: &str) -> &str {
    if is_local_redirect(s) { s } else { "/" }
}
#[cfg(not(any(test, feature = "test-util")))]
fn safe_return_to(s: &str) -> &str {
    if is_local_redirect(s) { s } else { "/" }
}

/// Unix seconds `ttl_secs` from now — how every short-lived cookie here
/// stamps its own deadline. `0` on a clock that can't produce one, which
/// [`expired`] then treats as already past.
fn deadline(ttl_secs: i64) -> u64 {
    SystemTime::now()
        .checked_add(Duration::from_secs(ttl_secs.max(0) as u64))
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `true` if `exp` is in the past. Used on the way *in*, so a cookie a
/// client chose to keep past its `Max-Age` is rejected on its own terms.
fn expired(exp: u64) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    exp < now
}

fn build_session_cookie(
    headers: &HeaderMap,
    cipher: &gitquarry_core::crypto::Cipher,
    subject: SessionSubject,
    epoch: u64,
) -> Result<(axum::http::HeaderName, String)> {
    let session = SessionCookie {
        subject,
        exp: deadline(SESSION_TTL.as_secs() as i64),
        epoch,
    };
    let sealed = cipher.seal(
        SESSION_COOKIE,
        &serde_json::to_vec(&session)?,
        gitquarry_core::crypto::CompressionStrategy::None,
    )?;
    Ok(set_cookie(
        headers,
        SESSION_COOKIE,
        &B64.encode(sealed),
        SESSION_TTL.as_secs() as i64,
    ))
}

/// A fresh anchor value for [`LOGIN_CSRF_COOKIE`], plus the `Set-Cookie`
/// that plants it. The value is a sealed blob rather than raw randomness
/// only because the cipher is already at hand and seals with a random
/// nonce, so every call produces a distinct, unguessable string — nothing
/// here ever reads the plaintext back, the value is opaque by design.
fn new_login_csrf(
    headers: &HeaderMap,
    cipher: &gitquarry_core::crypto::Cipher,
) -> Result<(String, (axum::http::HeaderName, String))> {
    let sealed = cipher.seal(
        LOGIN_CSRF_COOKIE,
        &deadline(LOGIN_CSRF_TTL_SECS).to_le_bytes(),
        gitquarry_core::crypto::CompressionStrategy::None,
    )?;
    let value = B64.encode(sealed);
    let cookie = set_cookie(headers, LOGIN_CSRF_COOKIE, &value, LOGIN_CSRF_TTL_SECS);
    Ok((value, cookie))
}

/// `true` if at least one user has a password set — what [`handler_login`]
/// checks to decide whether the password form is worth showing at all, and
/// what the admin-page gate checks to know whether *some* login method
/// exists (alongside `ctx.oidc`). Fails closed (`false`) on a fetch error,
/// same as the private-repo visibility check.
///
/// Reads the small per-bucket counter (`access::password_login_count`)
/// instead of fetching every user — `None` (counter object doesn't exist
/// yet) means the same thing as `Some(0)` here: nobody has a password.
pub(crate) async fn password_login_available(ctx: &AppContext) -> bool {
    matches!(
        gitquarry_core::auth::access::password_login_count(ctx.backend().as_ref(), &ctx.cipher).await,
        Ok(Some(count)) if count > 0
    )
}

/// `true` if the admin/repo-settings gates should actually enforce — what
/// `crate::settings::web::require_admin` and
/// `crate::webui::repo_settings::require_repo_authority` read before
/// deciding whether to check anything at all.
///
/// Deliberately NOT [`password_login_available`], even though the happy
/// paths agree: that one answers "is the password form worth rendering",
/// where collapsing a fetch error into `false` is harmless. Here `false`
/// means *skip the gate*, so the same collapse would turn a transient
/// backend blip into anonymous access to `/_settings`, `/_users`,
/// `/_audit` and every repo's settings page for as long as it lasted. An
/// unreadable counter is "can't tell", and can't-tell has to keep the gate
/// closed — only a counter that genuinely reads as zero (or absent,
/// meaning nobody ever set a password) leaves a fresh instance open, which
/// is the documented bootstrap behavior this preserves.
pub(crate) async fn login_enforced(ctx: &AppContext) -> bool {
    if ctx.oidc.is_some() {
        return true;
    }
    match gitquarry_core::auth::access::password_login_count(ctx.backend().as_ref(), &ctx.cipher).await {
        Ok(Some(count)) => count > 0,
        Ok(None) => false,
        Err(_) => true,
    }
}

/// `GET /_login`: the shared landing page. Straight to the IdP with no
/// intermediate page when OIDC is the only method configured (unchanged
/// behavior from before password login existed); otherwise a page with
/// whichever of the password form / OIDC link actually apply.
pub(crate) async fn handler_login(
    State(ctx): State_,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let return_to = params
        .get("return_to")
        .map(|s| safe_return_to(s).to_string())
        .unwrap_or_else(|| "/".to_string());

    if !password_login_available(&ctx).await {
        return handler_login_oidc(State(ctx), Query(params), headers).await;
    }

    // A fresh anchor per visit rather than reusing whatever the request
    // already carried: this page is where the token is handed out, so
    // rotating it here keeps a stale one from staying valid indefinitely.
    let (anchor, (cookie_header, cookie_value)) = new_login_csrf(&headers, &ctx.cipher)?;
    let body = html! {
        (login_form(locale, &return_to, "", None, &crate::webui::csrf_token(&anchor)))
        @if ctx.oidc.is_some() {
            p { a href=(format!("/_login_oidc?return_to={}", url_encode(&return_to))) { (gitquarry_i18n::web::auth::login::log_in_with_oidc_link(locale)) } }
        }
    }
    .into_string();
    let mut resp = Html(layout(
        "login",
        "",
        &body,
        "",
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response();
    resp.headers_mut().insert(
        cookie_header,
        cookie_value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    Ok(resp)
}

/// The password-login form — shared by the GET landing page (`error: None`)
/// and the POST handler on a rejected attempt, so a wrong username/password
/// or a lockout re-shows this same form (username preserved, password left
/// blank) instead of losing it to a separate error page. Both credential
/// fields get marked invalid together on error, never just one — same
/// "don't tell a guesser which half was wrong" reasoning `handler_login_local`
/// already documents for its constant-time hash check.
fn login_form(
    locale: gitquarry_i18n::Locale,
    return_to: &str,
    username: &str,
    error: Option<&str>,
    csrf: &str,
) -> maud::Markup {
    let field_class = if error.is_some() { "field invalid" } else { "field" };
    html! {
        form method="post" action="/_login" {
            input type="hidden" name="csrf" value=(csrf);
            input type="hidden" name="return_to" value=(return_to);
            div class=(field_class) {
                label { (gitquarry_i18n::web::auth::login::username_label(locale)) }
                input type="text" name="username" size="30" value=(username) autofocus;
            }
            div class=(field_class) {
                label { (gitquarry_i18n::web::auth::login::password_label(locale)) }
                input type="password" name="password" size="30";
                @if let Some(msg) = error {
                    div class="field-error" { (msg) }
                }
            }
            p { button type="submit" { (gitquarry_i18n::web::auth::login::log_in_button(locale)) } }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct LocalLoginForm {
    username: String,
    password: String,
    #[serde(default)]
    return_to: String,
    #[serde(default)]
    csrf: String,
}

/// `POST /_login`: the password form submit. Always runs
/// [`password::verify_password`] even when the username doesn't match
/// any user, against a fixed dummy hash — so a wrong username doesn't return
/// faster than a wrong password would and give a guesser a signal either way.
/// Rate-limited per submitted username (see [`gitquarry_core::app::LoginAttempts`]):
/// checked before Argon2 even runs, so a locked-out username costs nothing
/// extra per request either.
pub(crate) async fn handler_login_local(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<LocalLoginForm>,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    // Before anything else, including the lockout check: a cross-site form
    // must not be able to drive this endpoint at all. Logging someone into
    // an account the attacker controls is the whole attack — everything the
    // victim then does happens inside it.
    crate::webui::require_csrf_for(&headers, LOGIN_CSRF_COOKIE, &form.csrf)?;
    // A rejected attempt re-renders this same form, so it needs a token the
    // retry will still pass: the anchor cookie is unchanged and still on the
    // request, so derive from that rather than rotating mid-flow.
    let csrf = get_cookie(&headers, LOGIN_CSRF_COOKIE)
        .map(|value| crate::webui::csrf_token(&value))
        .unwrap_or_default();
    // Piled-up failures slow this username's attempts down rather than
    // locking it: a lockout keyed on a name anyone can type is a denial of
    // service against that name's real owner. See `LoginAttempts::penalty`.
    let penalty = ctx.login_attempts.penalty(&form.username);
    if !penalty.is_zero() {
        tokio::time::sleep(penalty).await;
    }

    let user = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, &form.username)
        .await?
        .0;
    let password_ok = password::verify_password(
        &form.password,
        user.as_ref()
            .and_then(|u| u.password_hash.as_deref())
            .unwrap_or(DUMMY_HASH.as_str()),
    );
    let Some(user) = user.filter(|u| u.password_hash.is_some() && password_ok) else {
        ctx.login_attempts.record_failure(&form.username);
        let body = login_form(
            locale,
            &form.return_to,
            &form.username,
            Some(&gitquarry_i18n::shared::auth::wrong_username_or_password(locale)),
            &csrf,
        )
        .into_string();
        return Ok((
            axum::http::StatusCode::BAD_REQUEST,
            Html(layout(
                "login",
                "",
                &body,
                "",
                effective_theme(&ctx, &headers).await,
                effective_density(&ctx, &headers).await,
                locale,
            )),
        )
            .into_response());
    };
    ctx.login_attempts.clear(&user.username);

    let return_to = safe_return_to(&form.return_to).to_string();

    if user.totp_enabled {
        // Password alone isn't enough for this account — hold off on the
        // real session cookie until `/_login_totp` also verifies a code.
        let pending = PendingTotpCookie {
            username: user.username,
            return_to,
            exp: deadline(PENDING_TOTP_TTL_SECS),
        };
        let sealed = ctx.cipher.seal(
            PENDING_TOTP_COOKIE,
            &serde_json::to_vec(&pending)?,
            gitquarry_core::crypto::CompressionStrategy::None,
        )?;
        let (header, value) = set_cookie(
            &headers,
            PENDING_TOTP_COOKIE,
            &B64.encode(sealed),
            PENDING_TOTP_TTL_SECS,
        );
        let mut resp = Redirect::to("/_login_totp").into_response();
        resp.headers_mut().insert(
            header,
            value
                .parse()
                .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
        );
        return Ok(resp);
    }

    let (header, value) = build_session_cookie(
        &headers,
        &ctx.cipher,
        SessionSubject::Username(user.username.clone()),
        user.session_epoch,
    )?;
    let mut resp = Redirect::to(&return_to).into_response();
    resp.headers_mut().insert(
        header,
        value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    Ok(resp)
}

/// `GET /_login_totp`: the "enter your code" interstitial, only meaningful
/// right after a password just verified for a 2FA-enabled user. Without a
/// live pending cookie (expired, or navigated here directly) this just sends
/// the browser back to the start rather than showing a dead-end form.
pub(crate) async fn handler_login_totp_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let Some(pending) = get_cookie(&headers, PENDING_TOTP_COOKIE) else {
        return Ok(Redirect::to("/_login").into_response());
    };
    // Anchored on the pending cookie rather than the session, same shape
    // `/_login` uses with `LOGIN_CSRF_COOKIE`: this step finishes the very
    // login flow that token exists to protect, and no session exists yet.
    let csrf = crate::webui::csrf_token(&pending);
    let locale = effective_locale(&ctx, &headers).await;
    let body = html! {
        form method="post" action="/_login_totp" {
            input type="hidden" name="csrf" value=(csrf);
            div class="field" {
                label { (gitquarry_i18n::web::auth::login::code_label(locale)) }
                input name="code" size="10" autofocus;
            }
            p { button type="submit" { (gitquarry_i18n::web::auth::login::verify_button(locale)) } }
        }
    }
    .into_string();
    Ok(Html(layout(
        "two-factor verification",
        "",
        &body,
        "",
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

#[derive(Deserialize)]
pub(crate) struct TotpLoginForm {
    code: String,
    #[serde(default)]
    csrf: String,
}

/// `POST /_login_totp`: verifies `code` against the pending cookie's
/// username's stored secret. Rate-limited via the *same* [`gitquarry_core::app::LoginAttempts`]
/// keyed by that username — a code-guessing attempt after a correct password
/// counts against the exact same lockout bucket a password-guessing attempt
/// would, no second lockout mechanism needed. Re-fetches the user fresh
/// rather than trusting anything from the cookie beyond the username (same
/// "the store is what's actually trusted" doctrine as [`current_user`]): if
/// 2FA got disabled mid-flow, `totp_enabled` is checked against the current
/// record and this correctly fails instead of trusting a stale claim.
pub(crate) async fn handler_login_totp_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<TotpLoginForm>,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    // Same reasoning as `/_login`'s own check, one step later in the same
    // flow: a cross-site form that completes someone else's second factor
    // lands them inside an account they didn't choose.
    crate::webui::require_csrf_for(&headers, PENDING_TOTP_COOKIE, &form.csrf)?;
    let raw = get_cookie(&headers, PENDING_TOTP_COOKIE)
        .context(gitquarry_i18n::shared::auth::no_totp_login_in_progress(locale))
        .map_err(AppError::bad_request)?;
    let sealed = B64
        .decode(raw)
        .context(gitquarry_i18n::shared::auth::invalid_totp_cookie(locale))
        .map_err(AppError::bad_request)?;
    let plain = ctx
        .cipher
        .open(PENDING_TOTP_COOKIE, &sealed)
        .map_err(AppError::bad_request)?;
    let pending: PendingTotpCookie =
        serde_json::from_slice(&plain).context(gitquarry_i18n::shared::auth::corrupt_totp_cookie(locale))?;
    if expired(pending.exp) {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::no_totp_login_in_progress(locale)
        )));
    }

    let penalty = ctx.login_attempts.penalty(&pending.username);
    if !penalty.is_zero() {
        tokio::time::sleep(penalty).await;
    }

    let user = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, &pending.username)
        .await?
        .0;
    // The step the code matched, kept rather than discarded: a code is
    // valid across three 30s steps, so without spending the step here an
    // observed code stays usable for up to 90s after its owner logged in.
    let matched_step = user.as_ref().and_then(|u| {
        if !u.totp_enabled {
            return None;
        }
        let step = gitquarry_auth_crypto::totp::matching_step(u.totp_secret.as_deref()?, form.code.trim())?;
        (step > u.totp_last_step).then_some(step)
    });
    // Written before the session is built: if the CAS loses to another
    // login racing this one, that other login spent the step and this one
    // is a replay.
    let code_ok = match matched_step {
        Some(step) => gitquarry_core::auth::domain::user::record_totp_step(&ctx, &pending.username, step).await?,
        None => false,
    };
    // A live TOTP code always wins outright; only bother spending a
    // recovery code (one-time use — see `domain::user::consume_recovery_code`)
    // when the submitted string didn't already verify as one.
    let recovery_ok = !code_ok
        && gitquarry_core::auth::domain::user::consume_recovery_code(&ctx, &pending.username, form.code.trim()).await?;
    let Some(user) = user.filter(|_| code_ok || recovery_ok) else {
        ctx.login_attempts.record_failure(&pending.username);
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::incorrect_code(locale)
        )));
    };
    ctx.login_attempts.clear(&user.username);

    let (session_header, session_value) = build_session_cookie(
        &headers,
        &ctx.cipher,
        SessionSubject::Username(user.username),
        user.session_epoch,
    )?;
    let (clear_header, clear_value) = set_cookie(&headers, PENDING_TOTP_COOKIE, "", 0);
    let mut resp = Redirect::to(&pending.return_to).into_response();
    let resp_headers = resp.headers_mut();
    resp_headers.append(
        session_header,
        session_value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    resp_headers.append(
        clear_header,
        clear_value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    Ok(resp)
}

/// `GET /_login_oidc`: starts the IdP round trip (was the whole body of
/// `/_login` before local admin login existed).
pub(crate) async fn handler_login_oidc(
    State(ctx): State_,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let client = ctx
        .oidc
        .as_ref()
        .context(gitquarry_i18n::shared::oidc::oidc_not_configured(locale))
        .map_err(AppError::bad_request)?;

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let return_to = params
        .get("return_to")
        .map(|s| safe_return_to(s).to_string())
        .unwrap_or_else(|| "/".to_string());
    let flow = FlowCookie {
        csrf: csrf.secret().clone(),
        nonce: nonce.secret().clone(),
        pkce_verifier: pkce_verifier.secret().clone(),
        return_to,
        exp: deadline(FLOW_TTL_SECS),
    };
    let sealed = ctx.cipher.seal(
        FLOW_COOKIE,
        &serde_json::to_vec(&flow)?,
        gitquarry_core::crypto::CompressionStrategy::None,
    )?;
    let (header, value) = set_cookie(&headers, FLOW_COOKIE, &B64.encode(sealed), FLOW_TTL_SECS);

    let mut resp = Redirect::to(auth_url.as_str()).into_response();
    resp.headers_mut().insert(
        header,
        value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    Ok(resp)
}

#[derive(Deserialize)]
pub(crate) struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub(crate) async fn handler_callback(
    State(ctx): State_,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let client = ctx
        .oidc
        .as_ref()
        .context(gitquarry_i18n::shared::oidc::oidc_not_configured(locale))
        .map_err(AppError::bad_request)?;

    if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or_default();
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::oidc::oidc_provider_rejected_login(locale, &err, &desc)
        )));
    }
    let code = params
        .code
        .context(gitquarry_i18n::shared::oidc::missing_code_param(locale))
        .map_err(AppError::bad_request)?;
    let state = params
        .state
        .context(gitquarry_i18n::shared::oidc::missing_state_param(locale))
        .map_err(AppError::bad_request)?;

    let flow_raw = get_cookie(&headers, FLOW_COOKIE)
        .context(gitquarry_i18n::shared::oidc::no_oidc_login_in_progress(locale))
        .map_err(AppError::bad_request)?;
    let flow_sealed = B64
        .decode(flow_raw)
        .context(gitquarry_i18n::shared::oidc::invalid_oidc_flow_cookie(locale))
        .map_err(AppError::bad_request)?;
    let flow_plain = ctx
        .cipher
        .open(FLOW_COOKIE, &flow_sealed)
        .map_err(AppError::bad_request)?;
    let flow: FlowCookie =
        serde_json::from_slice(&flow_plain).context(gitquarry_i18n::shared::oidc::corrupt_oidc_flow_cookie(locale))?;
    if expired(flow.exp) {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::oidc::no_oidc_login_in_progress(locale)
        )));
    }

    if flow.csrf != state {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::oidc::oidc_state_mismatch(locale)
        )));
    }

    let http_client = openidconnect::reqwest::ClientBuilder::new()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .context(gitquarry_i18n::shared::oidc::failed_to_build_oidc_client(locale))?;

    let token_response = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier))
        .request_async(&http_client)
        .await
        .context(gitquarry_i18n::shared::oidc::code_token_exchange_failed(locale))?;

    let id_token = token_response
        .id_token()
        .context(gitquarry_i18n::shared::oidc::oidc_missing_id_token(locale))?;
    let nonce = Nonce::new(flow.nonce);
    let claims = id_token
        .claims(&client.id_token_verifier(), &nonce)
        .context(gitquarry_i18n::shared::oidc::failed_to_verify_id_token(locale))?;

    // Nothing short of an explicit `true` will do. Identity here is the
    // email and nothing else (`SessionSubject::Email`), so an IdP that
    // merely *omits* the claim — or one where anyone may self-register an
    // address — would otherwise let an attacker sign up as an existing
    // account's email and be handed that account.
    if claims.email_verified() != Some(true) {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::oidc::oidc_email_not_verified(locale)
        )));
    }
    let email = claims
        .email()
        .context(gitquarry_i18n::shared::oidc::id_token_missing_email_claim(locale))?
        .as_str()
        .to_string();

    // The epoch has to be stamped in at issue time, so the account behind
    // this verified email gets resolved here rather than only later in
    // `current_user`. No match is not an error: the cookie is simply
    // useless (`current_user` finds nobody), so `0` is as good a stamp as
    // any.
    let account = gitquarry_core::auth::access::resolve_by_email(ctx.backend().as_ref(), &ctx.cipher, &email)
        .await
        .ok()
        .flatten();

    // An account that enrolled TOTP keeps its second factor whichever door
    // the first one came through — the IdP proved the email, not that this
    // is the person who holds the authenticator.
    if let Some(user) = account.as_ref().filter(|u| u.totp_enabled) {
        let pending = PendingTotpCookie {
            username: user.username.clone(),
            return_to: flow.return_to.clone(),
            exp: deadline(PENDING_TOTP_TTL_SECS),
        };
        let sealed = ctx.cipher.seal(
            PENDING_TOTP_COOKIE,
            &serde_json::to_vec(&pending)?,
            gitquarry_core::crypto::CompressionStrategy::None,
        )?;
        let (pending_header, pending_value) = set_cookie(
            &headers,
            PENDING_TOTP_COOKIE,
            &B64.encode(sealed),
            PENDING_TOTP_TTL_SECS,
        );
        let (clear_header, clear_value) = set_cookie(&headers, FLOW_COOKIE, "", 0);
        let mut resp = Redirect::to("/_login_totp").into_response();
        let resp_headers = resp.headers_mut();
        resp_headers.append(
            pending_header,
            pending_value
                .parse()
                .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
        );
        resp_headers.append(
            clear_header,
            clear_value
                .parse()
                .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
        );
        return Ok(resp);
    }

    let epoch = account.map_or(0, |u| u.session_epoch);
    let (session_header, session_value) =
        build_session_cookie(&headers, &ctx.cipher, SessionSubject::Email(email), epoch)?;
    let (clear_header, clear_value) = set_cookie(&headers, FLOW_COOKIE, "", 0);

    let mut resp = Redirect::to(&flow.return_to).into_response();
    let resp_headers = resp.headers_mut();
    resp_headers.append(
        session_header,
        session_value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    resp_headers.append(
        clear_header,
        clear_value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    Ok(resp)
}

#[derive(Deserialize)]
pub(crate) struct LogoutForm {
    #[serde(default)]
    csrf: String,
}

/// `POST /_logout`, not `GET`: logging out is a state change, and as a
/// plain link any third-party page could trigger it (an `<img src>` was
/// enough). Harmless next to a real CSRF target, but it's a free denial of
/// service against someone else's session, and the form + token this now
/// needs is the same machinery every other mutation here already uses.
pub(crate) async fn handler_logout(headers: HeaderMap, Form(form): Form<LogoutForm>) -> Result<Response, AppError> {
    crate::webui::require_csrf(&headers, &form.csrf)?;
    let (header, value) = set_cookie(&headers, SESSION_COOKIE, "", 0);
    let mut resp = Redirect::to("/").into_response();
    resp.headers_mut()
        .insert(header, value.parse().expect("logout cookie always valid"));
    Ok(resp)
}

tokio::task_local! {
    /// Memoizes [`current_user`]'s result for the rest of the request it
    /// was first computed in — scoped alongside
    /// [`gitquarry_core::app::REQUEST_BACKEND`] by
    /// `webui::request_cache_middleware`. A single page render calls
    /// `current_user` many times over (a permission check, `topnav`,
    /// theme/locale/text-size/density prefs, ...); `RequestCache` already
    /// serves the repeat *raw* read from memory, but every one of those
    /// calls still paid to decrypt the cookie and deserialize the `User`
    /// again. This doesn't weaken the "never cached across requests"
    /// invariant [`current_user`]'s own doc comment describes — the memo
    /// never outlives the one request that computed it, same as
    /// `RequestCache` itself.
    pub(crate) static CURRENT_USER_MEMO: std::cell::RefCell<Option<Option<User>>>;
}

/// The [`User`] the current request's session cookie resolves to, or `None`
/// for anything short of a fully valid, unexpired session naming a user
/// that still exists — never an error. The cookie only proves "this email
/// was IdP-verified recently" or "this username's password matched at
/// login time"; the per-entity store (re-read every call, never cached
/// *across requests*) is still what actually grants access, so removing a
/// user (or clearing their `password_hash`) revokes them immediately even
/// with a live cookie — [`CURRENT_USER_MEMO`] only dedupes repeat calls
/// within the one request that's already using whatever answer this
/// function first gave it.
pub(crate) async fn current_user(ctx: &AppContext, headers: &HeaderMap) -> Option<User> {
    if let Ok(Some(memoized)) = CURRENT_USER_MEMO.try_with(|cell| cell.borrow().clone()) {
        return memoized;
    }
    let user = current_user_uncached(ctx, headers).await;
    // `try_with` rather than `with`: outside a request (a background sweep,
    // a test calling this directly with no scope set up) there's simply
    // nothing to memoize into, which is fine — it only ever saves a repeat
    // call within a request that scoped it.
    let _ = CURRENT_USER_MEMO.try_with(|cell| *cell.borrow_mut() = Some(user.clone()));
    user
}

async fn current_user_uncached(ctx: &AppContext, headers: &HeaderMap) -> Option<User> {
    let raw = get_cookie(headers, SESSION_COOKIE)?;
    let sealed = B64.decode(raw).ok()?;
    let plain = ctx.cipher.open(SESSION_COOKIE, &sealed).ok()?;
    let session: SessionCookie = serde_json::from_slice(&plain).ok()?;
    if expired(session.exp) {
        return None;
    }
    let user = match session.subject {
        SessionSubject::Email(email) => {
            gitquarry_core::auth::access::resolve_by_email(ctx.backend().as_ref(), &ctx.cipher, &email)
                .await
                .ok()?
        }
        SessionSubject::Username(username) => {
            gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, &username)
                .await
                .ok()?
                .0
        }
    }?;
    // The stored record is what's actually trusted, `session_epoch`
    // included: a cookie issued before the account's last credential change
    // names a user who still exists but whose sessions were revoked, which
    // is exactly the case a stateless cookie can't express on its own.
    (session.epoch == user.session_epoch).then_some(user)
}

/// Pre-rendered snippet for [`crate::webui::layout`]'s topnav. `/_login` always
/// works — local username/password login exists independent of OIDC — so
/// the link is shown whenever there's no active session, not gated on
/// `ctx.oidc`.
pub(crate) async fn topnav(ctx: &AppContext, headers: &HeaderMap) -> String {
    let locale = effective_locale(ctx, headers).await;
    match current_user(ctx, headers).await {
        Some(user) => {
            let u = url_encode(&user.username);
            // `<details><summary>` — click-to-toggle/keyboard support from
            // the browser itself, no JS (see base.css's `#topbar
            // details.dropdown` rules). The 4 links used to sit flat in the
            // nav; grouping them under one "account" trigger is the one
            // dropdown this pass adds.
            format!(
                " · <details class=\"dropdown\"><summary>{}{}</summary><div class=\"dropdown-menu\">\
                 <a href=\"/_profile/{u}\">{}</a>\
                 <a href=\"/_account/favorites\">{}</a>\
                 <a href=\"/_account/security\">{}</a>\
                 <a href=\"/_account/profile\">{}</a>\
                 <form method=\"post\" action=\"/_logout\" class=\"dropdown-form\">{}<button type=\"submit\">{}</button></form>\
                 </div></details>",
                crate::webui::identicon::account_avatar(&user.username, user.avatar_content_type.as_deref())
                    .into_string(),
                esc(&user.username),
                gitquarry_i18n::web::chrome::profile_tab(locale),
                gitquarry_i18n::web::chrome::favorites_tab(locale),
                gitquarry_i18n::web::chrome::security_tab(locale),
                gitquarry_i18n::web::chrome::account_settings_tab(locale),
                crate::webui::csrf_field(headers),
                gitquarry_i18n::web::auth::login::logout_link(locale)
            )
        }
        None => format!(
            " · <a href=\"/_login\">{}</a>",
            gitquarry_i18n::web::auth::login::login_link(locale)
        ),
    }
}
