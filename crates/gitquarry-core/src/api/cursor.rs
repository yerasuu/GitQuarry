//! Opaque cursor pagination for every `/_api/v1` listing (decision #144).
//!
//! A cursor is the listing's resume position sealed with the bucket's own
//! [`Cipher`], the associated data naming the listing it belongs to (its
//! *scope*, e.g. one token's scopes or one repo's refs). So a client can't
//! read or edit the position, and a cursor issued for one scope fails to
//! open under any other: replaying a cursor from repo A against repo B is an
//! `invalid_cursor`, never a page of B starting wherever A left off. The
//! scope is also kept inside the sealed payload and compared after opening,
//! so the binding holds even under a cipher that does not authenticate.
//!
//! The cursor only says where to resume; it grants nothing. Every page is
//! authorized from the request itself, as the first one was.

use super::error::{ApiError, ErrorCode};
use crate::app::AppContext;
use crate::crypto::{Cipher, CompressionStrategy};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const DEFAULT_LIMIT: u32 = 50;
/// The ceiling on `[api] max_page_size`, which lowers it per instance.
pub const MAX_LIMIT: u32 = 100;

/// Far above any cursor this module issues for a uid-sized position; it only
/// bounds the work a hostile `cursor=` can make the server do before it is
/// rejected.
const MAX_CURSOR_LEN: usize = 2048;

const CURSOR_AAD: &str = "gitquarry/api/v1/cursor";

#[derive(Serialize, Deserialize)]
struct Payload {
    v: u8,
    scope: String,
    pos: String,
}

fn aad(scope: &str) -> String {
    format!("{CURSOR_AAD}:{scope}")
}

/// Seals `position` into a cursor that only [`open`] with the same `scope`
/// accepts.
///
/// # Errors
///
/// [`ErrorCode::Internal`] if the cipher refuses to seal (its nonce budget,
/// or no system RNG).
pub fn seal(cipher: &Cipher, scope: &str, position: &str) -> Result<String, ApiError> {
    let payload = Payload {
        v: 1,
        scope: scope.to_string(),
        pos: position.to_string(),
    };
    let plain = serde_json::to_vec(&payload).map_err(|e| ApiError::internal(&anyhow::Error::from(e)))?;
    let sealed = cipher
        .seal(&aad(scope), &plain, CompressionStrategy::None)
        .map_err(|e| ApiError::internal(&e))?;
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

/// The position [`seal`] put into `cursor` for this same `scope`.
///
/// # Errors
///
/// [`ErrorCode::InvalidCursor`] for anything else: not base64, too long,
/// tampered with, sealed under another key, or issued for another scope.
/// One code for all of them, so a client probing cursors learns nothing.
pub fn open(cipher: &Cipher, scope: &str, cursor: &str) -> Result<String, ApiError> {
    let invalid = || {
        ApiError::new(ErrorCode::InvalidCursor, "the cursor is not valid for this listing")
            .with_hint("pass back `next_cursor` from the previous page unchanged, or omit `cursor` to start over")
    };
    if cursor.len() > MAX_CURSOR_LEN {
        return Err(invalid());
    }
    let sealed = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let plain = cipher.open(&aad(scope), &sealed).map_err(|_| invalid())?;
    let payload: Payload = serde_json::from_slice(&plain).map_err(|_| invalid())?;
    if payload.v != 1 || payload.scope != scope {
        return Err(invalid());
    }
    Ok(payload.pos)
}

/// `?cursor=` and `?limit=` of a listing request, checked at the edge.
/// `cursor` is kept raw: only the handler knows the scope to [`open`] it
/// under, via [`PageRequest::position`].
#[derive(Debug)]
pub struct PageRequest {
    cursor: Option<String>,
    limit: u32,
}

impl PageRequest {
    /// Between 1 and the instance's `[api] max_page_size`, never above
    /// [`MAX_LIMIT`]; [`DEFAULT_LIMIT`] or that maximum, whichever is
    /// smaller, when not given.
    pub fn limit(&self) -> usize {
        usize::try_from(self.limit).unwrap_or(usize::MAX)
    }

    /// Where to resume, `None` for the first page.
    ///
    /// # Errors
    ///
    /// As [`open`].
    pub fn position(&self, cipher: &Cipher, scope: &str) -> Result<Option<String>, ApiError> {
        self.cursor.as_deref().map(|c| open(cipher, scope, c)).transpose()
    }
}

impl FromRequestParts<Arc<AppContext>> for PageRequest {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, ctx: &Arc<AppContext>) -> Result<Self, Self::Rejection> {
        let max = ctx.api_limits.max_page_size();
        let default = DEFAULT_LIMIT.min(max);
        let mut cursor = None;
        let mut limit = default;
        for (key, value) in form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                // An empty `cursor=` is what a client building the URL from a
                // null `next_cursor` sends; it means the first page.
                "cursor" if !value.is_empty() => cursor = Some(value.into_owned()),
                "limit" => {
                    limit = value.parse().ok().filter(|n| (1..=max).contains(n)).ok_or_else(|| {
                        ApiError::new(ErrorCode::InvalidParameter, "`limit` must be a whole number")
                            .with_hint(format!("use 1 to {max}; omit it for {default}"))
                    })?;
                }
                _ => {}
            }
        }
        Ok(PageRequest { cursor, limit })
    }
}

/// One page of a listing. `next_cursor` is `null` on the last page.
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}
