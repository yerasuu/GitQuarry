//! Outbound proxy routes to `gitquarry-federation`'s own loopback API
//! (`docs/federation-binary-split-concept.md` section 3A). Two shapes:
//!
//! - **Byte passthrough** (`/_api/*`): no session concerns at all — a
//!   `Bearer` token in `Authorization`, if any, rides straight through
//!   unchanged; federation resolves it itself.
//! - **Session-gated** (`/_peers`, `/_account/tokens`,
//!   `/_remote_pulls/comment`): core validates session + CSRF + whatever
//!   page-specific gate applies *before* ever forwarding anything, then
//!   asserts the result to federation via `X-Gitquarry-User`/
//!   `X-Gitquarry-User-Uid`/`X-Gitquarry-Csrf` — federation trusts these
//!   unconditionally (see `gitquarry-federation`'s own `web::session`
//!   module doc comment), since it has no session state of its own to
//!   validate against.
//!
//! Every request also carries `X-Internal-Token`, matching
//! [`gitquarry_core::app::FederationLink::internal_token`] — federation's entire
//! router requires it (its own `web` module doc comment explains why: core
//! is the only legitimate caller of any route on that port).

use super::guards::require_login;
use super::{AppError, State_, csrf_field, require_csrf};
use crate::auth::web::login;
use crate::settings::web::require_admin_or_federation_moderator;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gitquarry_core::app::FederationLink;
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use std::net::SocketAddr;

const USER_HEADER: &str = "x-gitquarry-user";
const USER_UID_HEADER: &str = "x-gitquarry-user-uid";
const CSRF_HEADER: &str = "x-gitquarry-csrf";
/// The real client address — federation's own loopback bind never sees
/// anything but *this* process's address otherwise, since it's never
/// reachable except through this proxy (see this module's own doc
/// comment). Federation's `PeerRateLimiter` reads this in preference to
/// its own `ConnectInfo`, same "trust the reverse proxy's header" pattern
/// `X-Forwarded-For` always is.
const CLIENT_IP_HEADER: &str = "x-forwarded-for";

fn federation_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "federation is not configured on this instance",
    )
        .into_response()
}

/// Headers that describe *this specific hop* (connection framing, the
/// original host) rather than the request/response payload — stripped both
/// ways so forwarding doesn't leak or corrupt them, same reasoning any
/// reverse proxy applies.
pub fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection" | "host" | "content-length" | "transfer-encoding" | "keep-alive" | "upgrade"
    )
}

pub async fn send(
    federation: &FederationLink,
    method: reqwest::Method,
    path_and_query: &str,
    extra_headers: &[(&str, String)],
    body: Bytes,
) -> Response {
    let url = format!("{}{path_and_query}", federation.base_url.trim_end_matches('/'));
    let mut req = federation
        .http
        .request(method, &url)
        .signed(&federation.internal_token)
        .body(body);
    for (name, value) in extra_headers {
        req = req.header(*name, value);
    }
    let resp = match req.send().await {
        Ok(resp) => resp,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("couldn't reach federation: {e}")).into_response(),
    };
    let status = resp.status();
    let mut builder = Response::builder().status(status.as_u16());
    for (name, value) in resp.headers() {
        if !is_hop_by_hop(name) {
            builder = builder.header(name, value);
        }
    }
    let bytes = resp.bytes().await.unwrap_or_default();
    builder
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "malformed response from federation").into_response())
}

/// Forwards `Authorization` (a `Bearer` fine-grained token, if present) and
/// nothing else session-related — this group has no session concerns at
/// all, see this module's own doc comment.
pub fn passthrough_headers(headers: &HeaderMap) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| vec![("authorization", v.to_string())])
        .unwrap_or_default();
    out.extend(content_type_header(headers));
    out
}

/// The original request's `Content-Type`, forwarded verbatim whenever the
/// original body rides along unchanged too (every proxy here that doesn't
/// re-encode the body itself) — without this, a proxied
/// `application/x-www-form-urlencoded` POST arrives at federation with no
/// `Content-Type` at all, and axum's `Form` extractor there 415s on it.
pub fn content_type_header(headers: &HeaderMap) -> Vec<(&'static str, String)> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| vec![("content-type", v.to_string())])
        .unwrap_or_default()
}

/// `false` if `path_and_query`'s path half contains a `.`/`..` segment, in
/// raw or percent-encoded form. This is the one proxy here that forwards a
/// *client-controlled* path (the `/_api/repos/{*name}` wildcard capture)
/// rather than a fixed literal, and it forwards the raw URI — no
/// normalizing layer sits in front of the router. `reqwest` resolves the
/// concatenated URL with RFC 3986 `remove_dot_segments` before sending, so
/// `/_api/repos/../../_internal-out/peers` would otherwise climb out of the
/// `/_api/*` surface and land on a federation route that trusts the
/// loopback boundary as its only gate — with core's own `X-Internal-Token`
/// attached. Percent-encoded forms don't currently normalize, but they're
/// rejected too rather than left resting on that.
pub fn traversal_free(path_and_query: &str) -> bool {
    let path = path_and_query.split(['?', '#']).next().unwrap_or("");
    !path.split('/').any(|seg| {
        matches!(
            seg.to_ascii_lowercase().as_str(),
            "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
        )
    })
}

/// Same ceiling axum's `DefaultBodyLimit` puts on every extractor-read
/// body. These handlers take the raw `Request` instead, which that limit
/// never reaches, and `/_api/*` is open to anonymous callers — so without
/// this one request could make core buffer an arbitrarily large body.
const MAX_PROXIED_BODY_BYTES: usize = 2 * 1024 * 1024;

/// `Err` is a ready 413. A body that fails mid-stream for another reason
/// (client hung up) gets the same answer, since nobody is left to read it.
async fn capped_body(req: Request) -> Result<Bytes, Box<Response>> {
    axum::body::to_bytes(req.into_body(), MAX_PROXIED_BODY_BYTES)
        .await
        .map_err(|_| Box::new((StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response()))
}

pub(crate) async fn proxy_public(
    State(ctx): State_,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let Some(federation) = &ctx.federation else {
        return federation_unavailable();
    };
    let method = req.method().clone();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    if !traversal_free(&path_and_query) {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    let mut headers = passthrough_headers(req.headers());
    headers.push((CLIENT_IP_HEADER, addr.ip().to_string()));
    let body = match capped_body(req).await {
        Ok(body) => body,
        Err(resp) => return *resp,
    };
    let method = reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    send(federation, method, &path_and_query, &headers, body).await
}

/// `/_peers`: admin-or-`is_federation_moderator`-gated
/// (`docs/federation-binary-split-concept.md` section 6). Forwards
/// `X-Gitquarry-User` (audit/display only — federation never uses it for
/// an access decision, `/_peers` mutations don't touch per-repo
/// permissions) and `X-Gitquarry-Csrf` (so federation's rendered form
/// embeds a token core's own `require_csrf` will accept on the follow-up
/// POST).
pub(crate) async fn proxy_peers(State(ctx): State_, req: Request) -> Result<Response, AppError> {
    let headers = req.headers().clone();
    if let Some(resp) = require_admin_or_federation_moderator(&ctx, &headers, "/_peers").await {
        return Ok(resp);
    }
    let Some(federation) = &ctx.federation else {
        return Ok(federation_unavailable());
    };
    let username = login::current_user(&ctx, &headers)
        .await
        .map(|u| u.username)
        .unwrap_or_default();
    let mut extra = vec![(USER_HEADER, username), (CSRF_HEADER, session_csrf_value(&headers))];
    extra.extend(content_type_header(&headers));
    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let body = match capped_body(req).await {
        Ok(body) => body,
        Err(resp) => return Ok(*resp),
    };
    require_proxied_csrf(&headers, &method, &body)?;
    Ok(send(federation, method, "/_peers", &extra, body).await)
}

/// `/_account/tokens`: self-service, no username in the URL — always the
/// logged-in session's own tokens, same as every other `/_account/*`
/// section (see `crate::webui::account_settings_page`'s own doc comment).
/// Also forwards `X-Gitquarry-User-Uid` — federation's `PeerCredential`
/// storage is keyed by uid, not username (see `gitquarry-federation`'s own
/// `access::peer_tokens` doc comment).
pub(crate) async fn proxy_tokens(State(ctx): State_, req: Request) -> Result<Response, AppError> {
    let headers = req.headers().clone();
    let user = match require_login(&ctx, &headers, "/_account/tokens").await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    let Some(federation) = &ctx.federation else {
        return Ok(federation_unavailable());
    };
    let mut extra = vec![
        (USER_HEADER, user.username.clone()),
        (USER_UID_HEADER, user.uid.to_string()),
        (CSRF_HEADER, session_csrf_value(&headers)),
    ];
    extra.extend(content_type_header(&headers));
    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let body = match capped_body(req).await {
        Ok(body) => body,
        Err(resp) => return Ok(*resp),
    };
    require_proxied_csrf(&headers, &method, &body)?;
    Ok(send(federation, method, "/_account/tokens", &extra, body).await)
}

#[derive(serde::Deserialize)]
pub(crate) struct RemoteCommentForm {
    #[serde(default)]
    csrf: String,
    peer: String,
    repo: String,
    number: u64,
    #[serde(default)]
    body: String,
}

/// `POST /_remote_pulls/comment`: logged-in-only (same as before the
/// split), CSRF-checked *here* since federation has no session state to
/// validate a submitted token against — only core's own session cookie
/// can do that.
pub(crate) async fn proxy_remote_comment(
    State(ctx): State_,
    headers: HeaderMap,
    axum::extract::Form(form): axum::extract::Form<RemoteCommentForm>,
) -> Result<Response, AppError> {
    let user = match require_login(&ctx, &headers, "/_peers").await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let Some(federation) = &ctx.federation else {
        return Ok(federation_unavailable());
    };
    // Re-encoded as JSON, not forwarded verbatim (the original body was
    // already consumed by the `Form` extractor above to check `csrf`,
    // which federation's own `RemoteCommentForm` doesn't have a field for)
    // — so `Content-Type` is set explicitly here to match, instead of
    // `content_type_header` forwarding the original `urlencoded` value.
    let extra = vec![
        (USER_UID_HEADER, user.uid.to_string()),
        ("content-type", "application/json".to_string()),
    ];
    let body = serde_json::to_vec(&serde_json::json!({
        "peer": form.peer, "repo": form.repo, "number": form.number, "body": form.body,
    }))
    .unwrap_or_default();
    Ok(send(
        federation,
        reqwest::Method::POST,
        "/_remote_pulls/comment",
        &extra,
        Bytes::from(body),
    )
    .await)
}

/// The `csrf` field of a proxied `application/x-www-form-urlencoded` body,
/// read out of the already-buffered bytes so the body itself still rides
/// through to federation byte-for-byte (unlike
/// [`proxy_remote_comment`], which re-encodes and can afford axum's `Form`
/// extractor). Empty string when absent, which [`require_csrf`] rejects.
pub fn proxied_csrf(body: &Bytes) -> String {
    form_urlencoded::parse(body)
        .find(|(key, _)| key == "csrf")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

/// Rejects a proxied mutation whose `csrf` field doesn't match this
/// request's own session cookie. Federation renders the token into its own
/// forms ([`session_csrf_value`]) but never validates it — it has no
/// session state to validate against, so this side has to, on the way back
/// in. A non-POST is left alone: these proxies' GET half only renders a
/// page.
fn require_proxied_csrf(headers: &HeaderMap, method: &reqwest::Method, body: &Bytes) -> Result<(), AppError> {
    if method != reqwest::Method::POST {
        return Ok(());
    }
    require_csrf(headers, &proxied_csrf(body))
}

pub fn session_csrf_value(headers: &HeaderMap) -> String {
    // `csrf_field` renders a whole `<input>` element; the raw value is the
    // only part federation needs to embed in its own rendered forms.
    let field = csrf_field(headers);
    field
        .split("value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default()
        .to_string()
}
