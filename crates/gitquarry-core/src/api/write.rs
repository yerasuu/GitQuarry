//! What every `/_api/v1` endpoint that changes data takes (decision #144),
//! so `dry_run` and `Idempotency-Key` ([`super::idempotency`]) mean the
//! same thing on all of them.
//!
//! A write handler takes a [`WriteRequest`] as its last extractor (it reads
//! the body) and does its work inside [`WriteRequest::run`], branching on
//! the [`Mode`] it is handed. In [`Mode::DryRun`] the handler validates
//! everything it would validate for real, checks permissions the same way,
//! and answers with what it would do, but writes nothing: no bucket object,
//! no git ref, no event. A dry run never reads or writes an idempotency
//! key either, so it neither replays a stored answer nor takes the key.

use super::auth::ApiPrincipal;
use super::error::{ApiError, ErrorCode};
use super::idempotency;
use crate::app::AppContext;
use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::future::Future;

/// Whether a write happens or is only described.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Apply,
    /// `?dry_run=true`: validate and describe, write nothing.
    DryRun,
}

/// `?dry_run=`, read at the edge: only `true` and `false`, so a typo such as
/// `dry_run=ture` is refused instead of quietly running the write for real.
fn mode(query: Option<&str>) -> Result<Mode, ApiError> {
    let mut mode = Mode::Apply;
    for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key == "dry_run" {
            mode = match value.as_ref() {
                "true" => Mode::DryRun,
                "false" => Mode::Apply,
                _ => {
                    return Err(
                        ApiError::new(ErrorCode::InvalidParameter, "`dry_run` must be `true` or `false`")
                            .with_hint("omit it to apply the change"),
                    );
                }
            };
        }
    }
    Ok(mode)
}

#[derive(Debug)]
struct Idempotent {
    key: String,
    fingerprint: String,
}

/// One write request: its [`Mode`], its `Idempotency-Key` if it sent one,
/// and its raw body, which the handler parses itself inside [`Self::run`].
#[derive(Debug)]
pub struct WriteRequest {
    mode: Mode,
    idempotent: Option<Idempotent>,
    body: Bytes,
}

impl WriteRequest {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Runs the write `op` in this request's [`Mode`] and answers with what
    /// it returns. With an `Idempotency-Key`, `principal`'s token and the key
    /// decide whether `op` runs at all or a stored answer is replayed, as
    /// [`super::idempotency`] describes; `principal` must be the one the
    /// handler authorized the write for.
    pub async fn run<F, Fut>(self, ctx: &AppContext, principal: &ApiPrincipal, op: F) -> Response
    where
        F: FnOnce(Mode, Bytes) -> Fut,
        Fut: Future<Output = Result<Response, ApiError>>,
    {
        match (self.mode, self.idempotent) {
            (Mode::Apply, Some(idempotent)) => {
                let write = op(Mode::Apply, self.body);
                idempotency::run(
                    ctx,
                    &principal.token().uid,
                    &idempotent.key,
                    &idempotent.fingerprint,
                    write,
                )
                .await
            }
            (mode, _) => op(mode, self.body).await.unwrap_or_else(IntoResponse::into_response),
        }
    }
}

/// The body is read under axum's `DefaultBodyLimit`, the same cap every
/// other body extractor on the router has.
impl<S: Send + Sync> FromRequest<S> for WriteRequest {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let mode = mode(request.uri().query())?;
        let key = request
            .headers()
            .get(idempotency::HEADER)
            .map(idempotency::parse_key)
            .transpose()?;
        let method = request.method().clone();
        let uri = request.uri().clone();
        let body = Bytes::from_request(request, state).await.map_err(|rejection| {
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::new(ErrorCode::RequestTooLarge, "the request body is too large")
            } else {
                ApiError::new(ErrorCode::InvalidParameter, "the request body could not be read")
            }
        })?;
        let idempotent = key.map(|key| Idempotent {
            fingerprint: idempotency::fingerprint(&method, &uri, &body),
            key,
        });
        Ok(WriteRequest { mode, idempotent, body })
    }
}
