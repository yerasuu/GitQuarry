//! Raw cookie read/write — hand-rolled instead of pulling in `axum-extra`'s
//! cookie jar: parsing `name=value; name2=value2` is a couple of `split`s,
//! not worth a whole crate for. See [`crate::auth::web::login`] for why the
//! *contents* of the session cookie still get real crypto, just via the
//! `Cipher` the process already has.

use axum::http::HeaderMap;

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
pub(crate) fn is_https(headers: &HeaderMap) -> bool {
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
/// A TLS proxy that forgets the header would still downgrade every cookie
/// silently, so [`force_secure`] adds `Secure` back on every response once
/// the instance's own `public_url` says it is served over HTTPS.
/// Adds `Secure` to every `Set-Cookie` in `response` that lacks it. For an
/// instance whose `ServerSettings::public_url` is `https://`, where no cookie
/// may ever travel over plain HTTP whatever the proxy sends.
pub(crate) fn force_secure(response: &mut axum::response::Response) {
    let headers = response.headers_mut();
    let lines: Vec<String> = headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|line| {
            if line.split(';').any(|attr| attr.trim().eq_ignore_ascii_case("secure")) {
                line.to_string()
            } else {
                format!("{line}; Secure")
            }
        })
        .collect();
    headers.remove(axum::http::header::SET_COOKIE);
    for line in lines {
        if let Ok(value) = axum::http::HeaderValue::from_str(&line) {
            headers.append(axum::http::header::SET_COOKIE, value);
        }
    }
}

pub(crate) fn set_cookie(
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
