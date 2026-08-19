//! Trust boundary for every session-gated page this crate renders
//! (`/_peers`, `/_account/tokens`) — `gitquarry-core` validates
//! session + CSRF + the admin/moderator gate *before* ever proxying here
//! (`docs/federation-binary-split-concept.md` section 3A), and forwards
//! what its own session already knows via these headers. Federation never
//! re-derives or re-validates any of it: if the loopback + `internal_token`
//! boundary was crossed at all, whatever core asserts here is trusted
//! as-is, same as `X-Internal-Token` itself is trusted by `core_client`'s
//! own counterpart middleware.

use axum::http::HeaderMap;
use gitquarry_wire::Uid;

#[cfg(any(test, feature = "test-util"))]
pub const USER_HEADER: &str = "x-gitquarry-user";
#[cfg(not(any(test, feature = "test-util")))]
const USER_HEADER: &str = "x-gitquarry-user";
#[cfg(any(test, feature = "test-util"))]
pub const USER_UID_HEADER: &str = "x-gitquarry-user-uid";
#[cfg(not(any(test, feature = "test-util")))]
const USER_UID_HEADER: &str = "x-gitquarry-user-uid";
#[cfg(any(test, feature = "test-util"))]
pub const CSRF_HEADER: &str = "x-gitquarry-csrf";
#[cfg(not(any(test, feature = "test-util")))]
const CSRF_HEADER: &str = "x-gitquarry-csrf";

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// The session's username — `None` only if this handler was somehow
/// reached without going through core's proxy at all (shouldn't happen
/// given the loopback bind + shared secret, but every caller still treats
/// it as a hard failure rather than assuming a default identity).
#[cfg(any(test, feature = "test-util"))]
pub fn forwarded_user(headers: &HeaderMap) -> Option<String> {
    header_str(headers, USER_HEADER).map(str::to_string)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn forwarded_user(headers: &HeaderMap) -> Option<String> {
    header_str(headers, USER_HEADER).map(str::to_string)
}

/// The session user's own uid — needed for `domain::peer_credential::*`,
/// which is keyed by uid rather than username (see that module's own doc
/// comment on why).
#[cfg(any(test, feature = "test-util"))]
pub fn forwarded_user_uid(headers: &HeaderMap) -> Option<Uid> {
    header_str(headers, USER_UID_HEADER).map(|s| Uid::from(s.to_string()))
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn forwarded_user_uid(headers: &HeaderMap) -> Option<Uid> {
    header_str(headers, USER_UID_HEADER).map(|s| Uid::from(s.to_string()))
}

/// Opaque CSRF token value to embed verbatim in a rendered form's hidden
/// field — federation never validates this itself (it has no session
/// state to validate against); core re-checks it on the way back in when
/// the resulting POST is proxied. Empty string if absent, which just
/// produces a form core's own CSRF check will reject — fails closed, not
/// open.
#[cfg(any(test, feature = "test-util"))]
pub fn forwarded_csrf(headers: &HeaderMap) -> String {
    header_str(headers, CSRF_HEADER).unwrap_or_default().to_string()
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn forwarded_csrf(headers: &HeaderMap) -> String {
    header_str(headers, CSRF_HEADER).unwrap_or_default().to_string()
}

/// Minimal HTML-escaping for the handful of user-controlled strings these
/// hand-rolled pages interpolate (peer names/URLs, token labels, usernames)
/// — this crate doesn't depend on a templating engine (no page-chrome to
/// integrate with once proxied byte-for-byte from core, see this module's
/// own doc comment), so every interpolated value must go through this.
#[cfg(any(test, feature = "test-util"))]
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
