//! URLs into core's loopback services (federation, tracker, artifacts) whose
//! path carries client-controlled segments. Each of those listeners takes
//! `X-Internal-Token` as its only gate, so a forwarded path that leaves the
//! prefix its proxy chose reaches routes no client was meant to call.
//!
//! Concatenating a client path onto the base URL is not enough to stay
//! inside that prefix: `url`, the parser under `reqwest`, treats `\` as `/`
//! in an http(s) path and removes `.`/`..` segments, raw or written as
//! `%2e` ([WHATWG URL: path state](https://url.spec.whatwg.org/#path-state)),
//! so `x\..\..\_internal-out\peers` is one segment to a `/` split and three
//! levels up to the listener. Here every segment is checked on its own and
//! appended with [`reqwest::Url::path_segments_mut`], which percent-encodes
//! `/`, `\` and `%` inside a segment
//! ([`PathSegmentsMut::push`](https://docs.rs/url/2.5.8/url/struct.PathSegmentsMut.html#method.push)),
//! and the result is parsed once more to prove the path `reqwest` sends is
//! the one built.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use std::borrow::Cow;

#[derive(Debug)]
pub(crate) enum LoopbackUrlError {
    /// The configured base URL doesn't parse or can't carry a path: the
    /// operator's error, never the client's.
    Base,
    /// A segment the listener could read as something else: empty, a dot
    /// segment, or holding a separator, `%`, NUL or another control
    /// character once percent-decoded. Answered 400, the status every proxy
    /// here gives a path it refuses to forward.
    Segment,
}

impl IntoResponse for LoopbackUrlError {
    fn into_response(self) -> Response {
        match self {
            Self::Base => (StatusCode::BAD_GATEWAY, "loopback service URL is invalid").into_response(),
            Self::Segment => (StatusCode::BAD_REQUEST, "invalid path").into_response(),
        }
    }
}

/// `base_url` + `prefix` + `segments`, each segment an already-decoded
/// value (an axum `Path` capture, a literal) that becomes exactly one path
/// segment, its `/`, `?` or `#` percent-encoded rather than interpreted.
///
/// # Errors
///
/// [`LoopbackUrlError::Segment`] for a segment `check_decoded` refuses,
/// [`LoopbackUrlError::Base`] for an unusable `base_url`.
pub(crate) fn from_segments(base_url: &str, prefix: &[&str], segments: &[&str]) -> Result<Url, LoopbackUrlError> {
    build(
        base_url,
        prefix,
        segments.iter().map(|s| check_decoded(s).map(Cow::Borrowed)),
        None,
    )
}

/// `base_url` + `prefix` + `raw_tail` + `?query`, where `raw_tail` is the
/// still percent-encoded path a client sent (from `Uri::path`, a leading
/// `/` optional) and `query` its raw query, forwarded as is: a query can't
/// move the path. Legitimate percent-encoding survives as the same decoded
/// value.
///
/// # Errors
///
/// [`LoopbackUrlError::Segment`] when any segment of `raw_tail` holds a raw
/// byte outside printable ASCII (a `\` included), a malformed `%` escape,
/// non-UTF-8 once decoded, or what `check_decoded` refuses — `%2e%2e`,
/// `%2F`, `%5c`, `%252e` and their case variants among them;
/// [`LoopbackUrlError::Base`] for an unusable `base_url`.
pub(crate) fn from_raw_tail(
    base_url: &str,
    prefix: &[&str],
    raw_tail: &str,
    query: Option<&str>,
) -> Result<Url, LoopbackUrlError> {
    let raw_tail = raw_tail.strip_prefix('/').unwrap_or(raw_tail);
    let segments = (!raw_tail.is_empty())
        .then(|| raw_tail.split('/'))
        .into_iter()
        .flatten()
        .map(|raw| decode(raw).map(Cow::Owned));
    build(base_url, prefix, segments, query)
}

fn build<'a>(
    base_url: &str,
    prefix: &[&str],
    tail: impl Iterator<Item = Result<Cow<'a, str>, LoopbackUrlError>>,
    query: Option<&str>,
) -> Result<Url, LoopbackUrlError> {
    let mut url = Url::parse(base_url).map_err(|_| LoopbackUrlError::Base)?;
    {
        let mut path = url.path_segments_mut().map_err(|()| LoopbackUrlError::Base)?;
        path.pop_if_empty();
        for segment in prefix {
            path.push(check_decoded(segment)?);
        }
    }
    let prefix_path = url.path().to_owned();
    let mut expected_segments = segment_count(&url);
    {
        let mut path = url.path_segments_mut().map_err(|()| LoopbackUrlError::Base)?;
        for segment in tail {
            path.push(&segment?);
            expected_segments += 1;
        }
    }
    url.set_query(query.filter(|q| !q.is_empty()));
    stays_under(&url, &prefix_path, expected_segments)?;
    Ok(url)
}

/// Non-empty segments only, so a root path (`/`, one empty segment) counts
/// as none and the count is the same before and after the first push.
fn segment_count(url: &Url) -> usize {
    url.path_segments()
        .map_or(0, |segments| segments.filter(|s| !s.is_empty()).count())
}

/// Belt and braces over the per-segment checks: the built path still starts
/// with the prefix, has one segment per input (`push` silently drops a `.`
/// or `..`), and parses back to itself, so nothing in the serialization is
/// read differently when `reqwest` sends it.
fn stays_under(url: &Url, prefix_path: &str, expected_segments: usize) -> Result<(), LoopbackUrlError> {
    let path = url.path();
    let under_prefix = path
        .strip_prefix(prefix_path)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || prefix_path.ends_with('/'));
    let reparsed = Url::parse(url.as_str()).is_ok_and(|again| again.path() == path);
    if under_prefix && segment_count(url) == expected_segments && reparsed {
        Ok(())
    } else {
        Err(LoopbackUrlError::Segment)
    }
}

/// One raw segment of a request path, percent-decoded. A raw `\` (which the
/// WHATWG parser would turn into `/`) or any other byte a client should
/// have percent-encoded is refused before decoding.
fn decode(raw: &str) -> Result<String, LoopbackUrlError> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if !b.is_ascii_graphic() || b == b'\\' {
            return Err(LoopbackUrlError::Segment);
        }
        if b == b'%' {
            let (Some(high), Some(low)) = (hex_value(bytes.get(i + 1)), hex_value(bytes.get(i + 2))) else {
                return Err(LoopbackUrlError::Segment);
            };
            out.push(high << 4 | low);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).map_err(|_| LoopbackUrlError::Segment)?;
    check_decoded(&decoded)?;
    Ok(decoded)
}

fn hex_value(digit: Option<&u8>) -> Option<u8> {
    char::from(*digit?).to_digit(16).and_then(|v| u8::try_from(v).ok())
}

/// A decoded segment that can only ever be one literal path segment.
/// `%` is refused rather than re-encoded so a double-encoded `%252e%252e`
/// can't become `..` at a listener that decodes once more; repo names
/// (`gitquarry_core::validate::sanitize_path`) never hold one.
fn check_decoded(segment: &str) -> Result<&str, LoopbackUrlError> {
    let refused = segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.chars().any(|c| matches!(c, '/' | '\\' | '%') || c.is_control());
    if refused {
        Err(LoopbackUrlError::Segment)
    } else {
        Ok(segment)
    }
}
