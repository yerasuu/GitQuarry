use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::borrow::Cow;
use tracing::error;

/// The machine-readable half of an [`ApiError`]. Clients branch on
/// [`Self::as_str`], so a variant's string never changes once released; a
/// new failure gets a new variant instead of reusing a close one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// No `Authorization: Bearer` credential at all.
    Unauthenticated,
    /// A bearer credential that is malformed, unknown, revoked, expired, or
    /// whose owner no longer exists. Not split further, so a caller probing
    /// secrets learns nothing about which of those it hit.
    InvalidToken,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    /// A `cursor` this server did not issue for this listing.
    InvalidCursor,
    InvalidParameter,
    /// A request body over the size a write accepts.
    RequestTooLarge,
    /// An `Idempotency-Key` this token already used for a different request.
    IdempotencyKeyReused,
    /// An `Idempotency-Key` whose first request is still running.
    IdempotencyKeyInUse,
    /// Over a rate limit: the token's own, or its client address's for
    /// failed credentials. `Retry-After` says when to try again.
    RateLimited,
    /// The answer is over the instance's `[api] max_response_bytes`; a
    /// smaller `limit` or a narrower path asks for less.
    ResponseTooLarge,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unauthenticated => "unauthenticated",
            ErrorCode::InvalidToken => "invalid_token",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotFound => "not_found",
            ErrorCode::MethodNotAllowed => "method_not_allowed",
            ErrorCode::InvalidCursor => "invalid_cursor",
            ErrorCode::InvalidParameter => "invalid_parameter",
            ErrorCode::RequestTooLarge => "request_too_large",
            ErrorCode::IdempotencyKeyReused => "idempotency_key_reused",
            ErrorCode::IdempotencyKeyInUse => "idempotency_key_in_use",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::ResponseTooLarge => "response_too_large",
            ErrorCode::Internal => "internal",
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            ErrorCode::Unauthenticated | ErrorCode::InvalidToken => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            ErrorCode::InvalidCursor | ErrorCode::InvalidParameter | ErrorCode::ResponseTooLarge => {
                StatusCode::BAD_REQUEST
            }
            ErrorCode::RequestTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::IdempotencyKeyReused => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::IdempotencyKeyInUse => StatusCode::CONFLICT,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Every `/_api/v1` failure, answered as `{"code", "message", "hint"}` with
/// the status [`ErrorCode::status`] gives. `hint` is always present (`null`
/// when there is nothing to suggest) so a client never has to probe for it.
/// `message` and `hint` are for a human or an agent to read and may be
/// reworded; only `code` is part of the contract.
#[derive(Debug)]
pub struct ApiError {
    code: ErrorCode,
    message: Cow<'static, str>,
    hint: Option<Cow<'static, str>>,
}

#[derive(Serialize)]
struct Body<'a> {
    code: &'static str,
    message: &'a str,
    hint: Option<&'a str>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        ApiError {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<Cow<'static, str>>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Logs `err` and answers with a generic message: a storage or
    /// decryption failure can name keys and internals the caller has no
    /// business seeing.
    pub fn internal(err: &anyhow::Error) -> Self {
        error!(error = %err, "api request failed");
        ApiError::new(ErrorCode::Internal, "internal error")
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Body {
            code: self.code.as_str(),
            message: &self.message,
            hint: self.hint.as_deref(),
        };
        let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| br#"{"code":"internal"}"#.to_vec());
        let mut response = (
            self.code.status(),
            [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            bytes,
        )
            .into_response();
        // RFC 6750 section 3: a 401 names the scheme it wants, and a
        // rejected token says so with `error="invalid_token"`.
        let challenge = match self.code {
            ErrorCode::Unauthenticated => Some(r#"Bearer realm="gitquarry""#),
            ErrorCode::InvalidToken => Some(r#"Bearer realm="gitquarry", error="invalid_token""#),
            _ => None,
        };
        if let Some(challenge) = challenge {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
        }
        response
    }
}
