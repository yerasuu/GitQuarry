//! The loopback-IPC transport wrapper every core↔optional-module pair
//! shares verbatim: the `X-Internal-Token` header + [`crate::WIRE_VERSION_HEADER`]
//! pair every outbound call carries, and the axum middleware every loopback
//! listener enforces them with. Extracted here (not duplicated per crate)
//! after near-identical copies accumulated across `gitquarry-core::internal_api`
//! and each optional module's own `core_client.rs` — nothing about the wire
//! shape changes, this only stops the same ~40 lines from being free to
//! drift out of sync.

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Header carrying the shared secret every loopback call authenticates
/// with — same value every existing copy already used.
pub const INTERNAL_TOKEN_HEADER: &str = "x-internal-token";

pub fn token_matches(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(INTERNAL_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|got| got == expected)
}

/// `true` unless the caller's `gitquarry-wire` build doesn't match this
/// one's [`crate::WIRE_PROTOCOL_VERSION`] — checked so a mismatch surfaces
/// as one clear error at the door instead of a confusing serde failure deep
/// inside whichever route happens to get called first after a partial
/// upgrade (one side of a pair redeployed, not both).
pub fn wire_version_matches(headers: &HeaderMap) -> bool {
    headers
        .get(crate::WIRE_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|got| got == crate::WIRE_PROTOCOL_VERSION)
}

#[derive(Serialize)]
struct ApiError<'a> {
    error: &'a str,
}

/// A JSON `{"error": "..."}` body at `status` — the shape every existing
/// `/_internal/*`-style error response already used, standardized here so
/// a client's JSON-error-body parse works uniformly regardless of which
/// layer rejected the request.
pub fn json_error(status: StatusCode, message: &str) -> Response {
    let bytes = serde_json::to_vec(&ApiError { error: message }).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(axum::http::header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

/// The middleware every loopback listener in this codebase layers onto its
/// router: reject with 401 on a missing/wrong `X-Internal-Token`, 409 on a
/// wire-version mismatch, otherwise pass through unchanged.
pub async fn require_internal_token(expected: &str, req: Request, next: Next) -> Response {
    if !token_matches(req.headers(), expected) {
        return json_error(StatusCode::UNAUTHORIZED, "missing or invalid internal token");
    }
    if !wire_version_matches(req.headers()) {
        return json_error(
            StatusCode::CONFLICT,
            "wire protocol version mismatch — the two sides of this loopback call were built from different versions, redeploy both",
        );
    }
    next.run(req).await
}

/// Adds the [`INTERNAL_TOKEN_HEADER`] + [`crate::WIRE_VERSION_HEADER`] pair
/// every outbound loopback call needs — the one-liner every `core_client.rs`
/// repeated at every single call site before this existed.
pub trait SignedRequestBuilderExt {
    fn signed(self, internal_token: &str) -> Self;
}

impl SignedRequestBuilderExt for reqwest::RequestBuilder {
    fn signed(self, internal_token: &str) -> Self {
        self.header(INTERNAL_TOKEN_HEADER, internal_token)
            .header(crate::WIRE_VERSION_HEADER, crate::WIRE_PROTOCOL_VERSION)
    }
}
