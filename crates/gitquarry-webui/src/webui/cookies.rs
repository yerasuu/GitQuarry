//! Raw cookie read/write — hand-rolled instead of pulling in `axum-extra`'s
//! cookie jar: parsing `name=value; name2=value2` is a couple of `split`s,
//! not worth a whole crate for. See [`crate::auth::web::login`] for why the
//! *contents* of the session cookie still get real crypto, just via the
//! `Cipher` the process already has.

use axum::http::HeaderMap;

/// Reads one cookie's value out of the request's `Cookie` header.
#[cfg(any(test, feature = "test-util"))]
pub fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// `true` if this request reached us over HTTPS, per `X-Forwarded-Proto`.
///
/// The server itself only ever speaks plain HTTP (`axum::serve`, no TLS
/// here), so a TLS deployment always terminates in front of it and the
/// header is the only signal available. Trusting a client-supplied value
/// is fine for this one decision: the worst a forged `https` does is make
/// the sender's *own* cookie refuse to travel over plain HTTP.
#[cfg(any(test, feature = "test-util"))]
pub fn is_https(headers: &HeaderMap) -> bool {
    is_https_impl(headers)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn is_https(headers: &HeaderMap) -> bool {
    is_https_impl(headers)
}

fn is_https_impl(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        // A proxy chain may append: `https, http`. The first hop is the one
        // that faced the client.
        .and_then(|v| v.split(',').next())
        .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"))
}

/// Builds a `Set-Cookie` header value. `HttpOnly` (never readable from JS)
/// and `SameSite=Lax` (sent on top-level navigation, not cross-site
/// subrequests — enough for a login-redirect flow, blocks the usual CSRF
/// vector) always on.
///
/// `Secure` follows [`is_https`] rather than being hardcoded either way.
/// Unconditionally on would break every plain-HTTP deployment (the browser
/// would drop the session cookie and login would silently never stick),
/// which is why it was originally left off entirely; but this project
/// supports public ssh/git-HTTP now, so a deployment that *is* behind TLS
/// should not be handing its session cookie to a plain-HTTP request. Keyed
/// on the request, so both deployments get the right answer without a
/// config knob to set wrong. `max_age_secs = 0` clears the cookie (logout).
#[cfg(any(test, feature = "test-util"))]
pub fn set_cookie(headers: &HeaderMap, name: &str, value: &str, max_age_secs: i64) -> (axum::http::HeaderName, String) {
    set_cookie_impl(headers, name, value, max_age_secs)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn set_cookie(
    headers: &HeaderMap,
    name: &str,
    value: &str,
    max_age_secs: i64,
) -> (axum::http::HeaderName, String) {
    set_cookie_impl(headers, name, value, max_age_secs)
}

fn set_cookie_impl(
    headers: &HeaderMap,
    name: &str,
    value: &str,
    max_age_secs: i64,
) -> (axum::http::HeaderName, String) {
    let secure = if is_https(headers) { "; Secure" } else { "" };
    (
        axum::http::header::SET_COOKIE,
        format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={max_age_secs}"),
    )
}
