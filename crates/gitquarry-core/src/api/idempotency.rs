//! `Idempotency-Key` on `/_api/v1` writes (decision #144), after
//! [draft-ietf-httpapi-idempotency-key-header-08](https://datatracker.ietf.org/doc/html/draft-ietf-httpapi-idempotency-key-header-08),
//! so an agent that lost an answer can retry without doing the write twice.
//!
//! The contract a client relies on:
//!
//! - A key belongs to one token: two tokens, even of one owner, never see
//!   each other's keys.
//! - The first request with a key runs. A repeat with the same method, path,
//!   query and body gets the first answer back (status, `Content-Type`,
//!   `Location` and body) with `Idempotent-Replayed: true`, and nothing
//!   runs again. The same key with anything else is `422`
//!   `idempotency_key_reused`; while the first request is still running, a
//!   repeat is `409` `idempotency_key_in_use`.
//! - A `5xx` answer is not kept: the key is released and a retry runs the
//!   write again, since the client can't tell how far a failed one got any
//!   better than the server can.
//! - A key expires [`KEY_TTL`] after its first request; past that the same
//!   key starts a new write. [`run_periodic_sweep`] deletes expired keys.
//!
//! Records are an s3db entity in `.conf/api-idempotency`, sealed like
//! every bucket object. The record keeps only what the first answer
//! already gave this token, plus a hash of the request, never the request
//! body itself.

use super::error::{ApiError, ErrorCode};
use crate::app::AppContext;
use crate::storage::Backend;
use crate::storage::uid::Uid;
use anyhow::Result;
use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use s3db::{Cas, Entity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, warn};

pub const HEADER: &str = "idempotency-key";
/// Set to `true` on an answer replayed from a stored one.
pub const REPLAYED_HEADER: &str = "idempotent-replayed";
pub const KEY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a key stays `in use` without its first request finishing. Far
/// above any write this API runs; past it the server that took the key is
/// taken to have died mid-request, and a retry runs the write again rather
/// than being refused until the key expires.
const PENDING_LEASE: Duration = Duration::from_secs(5 * 60);

/// Same ceiling as the draft's examples and as most providers allow; the
/// key only ever reaches a hash, so this bounds work, not storage.
const MAX_KEY_LEN: usize = 255;

/// Above anything `[api] max_response_bytes` lets out; an answer this
/// large is sent but not kept, and its key is released.
const MAX_STORED_BODY: usize = super::limits::MAX_RESPONSE_BYTES_CEILING as usize;

const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Default, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/api-idempotency")]
struct IdempotencyRecord {
    /// [`record_name`] of the token and the key.
    #[serde(default)]
    #[entity(key)]
    name: String,
    /// [`fingerprint`] of the first request.
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    created_at: u64,
    /// Random per run, so the request that took the key can tell its own
    /// pending record from one a retry took over after [`PENDING_LEASE`].
    #[serde(default)]
    attempt: String,
    /// `None` while the first request is still running.
    #[serde(default)]
    response: Option<StoredResponse>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredResponse {
    #[serde(default)]
    status: u16,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    body: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// The header's key. The draft makes the field an RFC 9651 sf-string
/// (`"…"`, with `\"` and `\\` escapes); a bare token is accepted too, as
/// most clients send it that way. Either way printable ASCII only, 1 to
/// [`MAX_KEY_LEN`] characters.
///
/// # Errors
///
/// `invalid_parameter` for anything else.
pub(crate) fn parse_key(value: &HeaderValue) -> Result<String, ApiError> {
    let invalid = || {
        ApiError::new(ErrorCode::InvalidParameter, "the `Idempotency-Key` header is not valid").with_hint(format!(
            "send a quoted string of 1 to {MAX_KEY_LEN} printable ASCII characters, such as a UUID"
        ))
    };
    let raw = value.to_str().map_err(|_| invalid())?.trim();
    let key = match raw.strip_prefix('"') {
        Some(rest) => {
            let inner = rest.strip_suffix('"').ok_or_else(invalid)?;
            let mut key = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => match chars.next() {
                        Some(escaped @ ('"' | '\\')) => key.push(escaped),
                        _ => return Err(invalid()),
                    },
                    '"' => return Err(invalid()),
                    ' '..='~' => key.push(c),
                    _ => return Err(invalid()),
                }
            }
            key
        }
        None if raw.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\') => raw.to_string(),
        None => return Err(invalid()),
    };
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return Err(invalid());
    }
    Ok(key)
}

/// What makes two requests "the same" for a key: method, path, query and
/// body. Length-prefixed so no split of one into another collides.
pub(crate) fn fingerprint(method: &Method, uri: &Uri, body: &[u8]) -> String {
    let target = uri.path_and_query().map_or(uri.path(), |pq| pq.as_str());
    let mut hash = Sha256::new();
    for part in [method.as_str().as_bytes(), target.as_bytes(), body] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hex(&hash.finalize())
}

/// A hash, not the key itself: a client key may hold characters an object
/// key can't, and the token uid in it is what scopes keys per token.
fn record_name(token_uid: &Uid, key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(token_uid.as_str().as_bytes());
    hash.update([0]);
    hash.update(key.as_bytes());
    hex(&hash.finalize())
}

fn expired(record: &IdempotencyRecord, now: u64) -> bool {
    record.created_at.saturating_add(KEY_TTL.as_secs()) <= now
}

fn abandoned(record: &IdempotencyRecord, now: u64) -> bool {
    record.response.is_none() && record.created_at.saturating_add(PENDING_LEASE.as_secs()) <= now
}

/// Removes the record outright. Not `Collection::delete`, which keeps an
/// archived copy and a tombstone: an expired answer has to be gone.
async fn purge(backend: &dyn Backend, name: &str) -> Result<()> {
    backend.delete(&IdempotencyRecord::COLLECTION.key(name)?).await?;
    Ok(())
}

enum Begin {
    Run { attempt: String },
    Answer(Response),
}

/// Same number of CAS rounds as the other short races in this crate; two
/// requests on one key resolve in one.
const BEGIN_ATTEMPTS: usize = 3;

async fn begin(ctx: &AppContext, name: &str, fingerprint: &str) -> Result<Begin, ApiError> {
    let backend = ctx.backend.as_ref();
    for _ in 0..BEGIN_ATTEMPTS {
        let (current, etag) = IdempotencyRecord::COLLECTION
            .get::<IdempotencyRecord>(backend, &ctx.cipher, name)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        let now = now_secs();
        if let Some(record) = current.as_ref().filter(|r| !expired(r, now) && !abandoned(r, now)) {
            if record.fingerprint != fingerprint {
                return Err(ApiError::new(
                    ErrorCode::IdempotencyKeyReused,
                    "this Idempotency-Key was already used for a different request",
                )
                .with_hint("use a new key for a new request; repeat a request unchanged to get its answer again"));
            }
            return match &record.response {
                Some(stored) => Ok(Begin::Answer(replay(stored))),
                None => Err(ApiError::new(
                    ErrorCode::IdempotencyKeyInUse,
                    "the first request with this Idempotency-Key is still running",
                )
                .with_hint("retry the same request in a few seconds to get its answer")),
            };
        }
        let attempt = hex(&rand::random::<[u8; 16]>());
        let pending = IdempotencyRecord {
            name: name.to_string(),
            fingerprint: fingerprint.to_string(),
            created_at: now,
            attempt: attempt.clone(),
            response: None,
        };
        match IdempotencyRecord::COLLECTION
            .put(backend, &ctx.cipher, name, &pending, etag.as_deref())
            .await
            .map_err(|e| ApiError::internal(&e))?
        {
            Cas::Ok => return Ok(Begin::Run { attempt }),
            Cas::Conflict => {}
        }
    }
    Err(ApiError::new(
        ErrorCode::IdempotencyKeyInUse,
        "other requests with this Idempotency-Key keep racing this one",
    )
    .with_hint("retry the same request in a few seconds to get its answer"))
}

fn replay(stored: &StoredResponse) -> Response {
    let status = StatusCode::from_u16(stored.status).unwrap_or(StatusCode::OK);
    let mut response = (status, Body::from(stored.body.clone())).into_response();
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CONTENT_TYPE, stored.content_type.as_deref()),
        (header::LOCATION, stored.location.as_deref()),
    ] {
        if let Some(value) = value.and_then(|v| HeaderValue::from_str(v).ok()) {
            headers.insert(name, value);
        }
    }
    headers.insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
    response
}

fn header_string(response: &Response, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Writes `stored` over this attempt's pending record, or releases the key
/// when `stored` is `None`. Whatever happens here, the write itself has
/// already happened, so a failure is logged and the client still gets its
/// answer.
async fn finish(ctx: &AppContext, name: &str, attempt: &str, stored: Option<StoredResponse>) {
    let backend = ctx.backend.as_ref();
    let current = IdempotencyRecord::COLLECTION
        .get::<IdempotencyRecord>(backend, &ctx.cipher, name)
        .await;
    let (mut record, etag) = match current {
        Ok((Some(record), etag)) if record.attempt == attempt && record.response.is_none() => (record, etag),
        Ok(_) => {
            warn!(
                record = name,
                "idempotency key was taken over before its first request finished"
            );
            return;
        }
        Err(e) => {
            error!(record = name, error = %e, "couldn't read back an idempotency record");
            return;
        }
    };
    let Some(stored) = stored else {
        if let Err(e) = purge(backend, name).await {
            error!(record = name, error = %e, "couldn't release an idempotency key");
        }
        return;
    };
    record.response = Some(stored);
    match IdempotencyRecord::COLLECTION
        .put(backend, &ctx.cipher, name, &record, etag.as_deref())
        .await
    {
        Ok(Cas::Ok) => {}
        Ok(Cas::Conflict) => warn!(record = name, "idempotency record changed while storing its answer"),
        Err(e) => error!(record = name, error = %e, "couldn't store an idempotency answer"),
    }
}

/// Runs `write` once per token and key, as the module describes.
///
/// Reads and writes through the shared backend, not the request's cached
/// one, since the record is what concurrent requests race on.
pub(crate) async fn run<Fut>(ctx: &AppContext, token_uid: &Uid, key: &str, fingerprint: &str, write: Fut) -> Response
where
    Fut: Future<Output = Result<Response, ApiError>>,
{
    let name = record_name(token_uid, key);
    let attempt = match begin(ctx, &name, fingerprint).await {
        Ok(Begin::Run { attempt }) => attempt,
        Ok(Begin::Answer(response)) => return response,
        Err(e) => return e.into_response(),
    };
    let response = write.await.unwrap_or_else(IntoResponse::into_response);
    if response.status().is_server_error() {
        finish(ctx, &name, &attempt, None).await;
        return response;
    }
    let content_type = header_string(&response, header::CONTENT_TYPE);
    let location = header_string(&response, header::LOCATION);
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_STORED_BODY).await {
        Ok(bytes) => bytes,
        Err(e) => {
            finish(ctx, &name, &attempt, None).await;
            return ApiError::internal(&anyhow::Error::new(e)).into_response();
        }
    };
    let stored = std::str::from_utf8(&bytes).ok().map(|body| StoredResponse {
        status: parts.status.as_u16(),
        content_type,
        location,
        body: body.to_string(),
    });
    finish(ctx, &name, &attempt, stored).await;
    Response::from_parts(parts, Body::from(bytes))
}

/// Deletes every expired key, returning how many. A record that can't be
/// read or opened is left alone and logged: the sweep never deletes what it
/// could not check.
///
/// # Errors
///
/// Listing the collection failed.
pub async fn sweep_expired(ctx: &AppContext) -> Result<usize> {
    let backend = ctx.backend.as_ref();
    let now = now_secs();
    let mut purged = 0;
    for name in IdempotencyRecord::COLLECTION.list_names(backend).await? {
        match IdempotencyRecord::COLLECTION
            .get::<IdempotencyRecord>(backend, &ctx.cipher, &name)
            .await
        {
            Ok((Some(record), _)) if expired(&record, now) => match purge(backend, &name).await {
                Ok(()) => purged += 1,
                Err(e) => warn!(record = name, error = %e, "couldn't delete an expired idempotency key"),
            },
            Ok(_) => {}
            Err(e) => warn!(record = name, error = %e, "couldn't read an idempotency record"),
        }
    }
    Ok(purged)
}

/// Runs [`sweep_expired`] forever, hourly, so a key outlives [`KEY_TTL`]
/// by at most an hour. Meant to run detached (`tokio::spawn`), like the
/// other sweeps: a failure logs and waits for the next tick.
pub async fn run_periodic_sweep(ctx: Arc<AppContext>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(e) = sweep_expired(&ctx).await {
            warn!(error = %e, "idempotency key sweep failed");
        }
    }
}
