//! CSRF protection via double-submit-cookie: a token *derived* from the
//! session cookie, echoed back as a hidden form field.

use super::cookies::get_cookie;
use super::error::AppError;
use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

/// Hidden `<input>` carrying this request's CSRF token — double-submit-
/// cookie, derived from the session cookie rather than needing a second
/// one. An attacker's cross-site form can make the browser attach the
/// cookie automatically (`SameSite=Lax` already blocks most of that — see
/// [`super::cookies::set_cookie`] — this is defense in depth for what it
/// doesn't: older browsers, some subdomain-cookie edge cases), but can't
/// read the cookie's value (`HttpOnly`) to derive this field's value.
///
/// [`csrf_token`], not the cookie's own value: the token goes into the
/// markup of every authenticated page, so making it *be* the session
/// would put a live credential in the DOM, in view-source, and in
/// anything that caches or copies the response — and would hand full
/// account access to any injection that can read one element, which is
/// exactly what `HttpOnly` exists to prevent. A one-way digest is enough
/// here: an attacker who can't read the cookie can't compute the token,
/// and one who *can* read it doesn't need the token.
///
/// Empty string with no session: only logged-in-required actions get this
/// (see [`require_csrf`]) — there's nothing meaningful to protect on an
/// anonymous POST like the text-size toggle.
#[cfg(any(test, feature = "test-util"))]
pub fn csrf_field(headers: &HeaderMap) -> String {
    match get_cookie(headers, crate::auth::web::login::SESSION_COOKIE) {
        Some(session) => {
            let token = csrf_token(&session);
            maud::html! { input type="hidden" name="csrf" value=(token); }.into_string()
        }
        None => String::new(),
    }
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn csrf_field(headers: &HeaderMap) -> String {
    match get_cookie(headers, crate::auth::web::login::SESSION_COOKIE) {
        Some(session) => {
            let token = csrf_token(&session);
            maud::html! { input type="hidden" name="csrf" value=(token); }.into_string()
        }
        None => String::new(),
    }
}

/// The CSRF token for a given session-cookie value: a plain SHA256 of it,
/// hex. Deterministic (so it needs no server-side storage and survives a
/// restart, same as the sealed cookie itself) and one-way (so publishing it
/// in every form doesn't publish the session). No keying: this isn't
/// authenticating a message, it's proving the submitter could read a cookie
/// they can't read cross-origin.
#[cfg(any(test, feature = "test-util"))]
pub fn csrf_token(cookie_value: &str) -> String {
    csrf_token_impl(cookie_value)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn csrf_token(cookie_value: &str) -> String {
    csrf_token_impl(cookie_value)
}

fn csrf_token_impl(cookie_value: &str) -> String {
    let digest = Sha256::digest(cookie_value.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// [`require_csrf`] against an arbitrary cookie rather than the session.
///
/// `/_login` needs exactly this protection before any session exists — a
/// cross-site form that logs someone into an account the attacker controls
/// still gets that person working inside it — but has no session cookie to
/// derive a token from, so it carries its own short-lived one
/// (`crate::auth::web::login::LOGIN_CSRF_COOKIE`). Same double-submit shape
/// either way, just told which cookie is the anchor.
#[cfg(any(test, feature = "test-util"))]
pub fn require_csrf_for(headers: &HeaderMap, cookie_name: &str, submitted: &str) -> Result<(), AppError> {
    require_csrf_for_impl(headers, cookie_name, submitted)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn require_csrf_for(headers: &HeaderMap, cookie_name: &str, submitted: &str) -> Result<(), AppError> {
    require_csrf_for_impl(headers, cookie_name, submitted)
}

fn require_csrf_for_impl(headers: &HeaderMap, cookie_name: &str, submitted: &str) -> Result<(), AppError> {
    let expected = get_cookie(headers, cookie_name)
        .map(|value| csrf_token(&value))
        .unwrap_or_default();
    if expected.is_empty() || !ct_eq(submitted, &expected) {
        let locale = super::locale_pref::cookie_locale(headers).unwrap_or(gitquarry_i18n::Locale::DEFAULT);
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::invalid_csrf(locale)
        )));
    }
    Ok(())
}

/// Constant-time comparison — the token is a full-length digest (not a
/// low-entropy secret an online guesser could feasibly narrow down), but a
/// byte-at-a-time short-circuit costs nothing to avoid either.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Rejects a POST whose submitted `csrf` field doesn't match this request's
/// own session cookie (see [`csrf_field`]) — call after
/// [`super::guards::require_login`] (there's no session to compare against
/// otherwise) on every handler that mutates something on behalf of a
/// logged-in user.
#[cfg(any(test, feature = "test-util"))]
pub fn require_csrf(headers: &HeaderMap, submitted: &str) -> Result<(), AppError> {
    let expected = get_cookie(headers, crate::auth::web::login::SESSION_COOKIE)
        .map(|session| csrf_token(&session))
        .unwrap_or_default();
    if expected.is_empty() || !ct_eq(submitted, &expected) {
        // Sync-only fallback (no `ctx`, so no `effective_locale`'s
        // account-default lookup here): the explicit cookie override, else
        // the built-in default — same as every other locale resolution's
        // first tier, just missing the account-preference middle tier.
        let locale = super::locale_pref::cookie_locale(headers).unwrap_or(gitquarry_i18n::Locale::DEFAULT);
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::invalid_csrf(locale)
        )));
    }
    Ok(())
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn require_csrf(headers: &HeaderMap, submitted: &str) -> Result<(), AppError> {
    let expected = get_cookie(headers, crate::auth::web::login::SESSION_COOKIE)
        .map(|session| csrf_token(&session))
        .unwrap_or_default();
    if expected.is_empty() || !ct_eq(submitted, &expected) {
        // Sync-only fallback (no `ctx`, so no `effective_locale`'s
        // account-default lookup here): the explicit cookie override, else
        // the built-in default — same as every other locale resolution's
        // first tier, just missing the account-preference middle tier.
        let locale = super::locale_pref::cookie_locale(headers).unwrap_or(gitquarry_i18n::Locale::DEFAULT);
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::invalid_csrf(locale)
        )));
    }
    Ok(())
}
