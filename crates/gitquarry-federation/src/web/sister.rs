//! `/_federation/sister/*` — section 7's mutual handshake and ban-list
//! sync endpoints. Genuinely peer-to-peer (reached through the receiving
//! instance's own core proxy, same as `/_api/*`/`/_federation/gossip/*`),
//! authenticated with the shared secret [`crate::sister`]'s handshake
//! established, never the loopback `X-Internal-Token` (that only proves
//! "core forwarded this", not "this specific sister sent it").

use crate::sister::{BanEventWire, SisterBanEntry};
use crate::state::State_;
use crate::{access, sister};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

fn json_error(status: StatusCode, message: &str) -> Response {
    json_response(status, &serde_json::json!({ "error": message }))
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    Some(value.strip_prefix("Bearer ")?.trim().to_string())
}

#[derive(Deserialize)]
struct HandshakeRequest {
    instance_name: String,
    secret: String,
}

/// `POST /_federation/sister/handshake` — no bearer token yet, this call
/// is what *establishes* the shared secret in the first place. Anyone can
/// call it, but it only ever does anything if this side already has
/// `sister_requested` set for `instance_name` (`sister::receive_handshake`'s
/// own doc comment).
#[cfg(any(test, feature = "test-util"))]
pub async fn handler_handshake_post(State(state): State_, body: Bytes) -> Response {
    handler_handshake_post_impl(State(state), body).await
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) async fn handler_handshake_post(State(state): State_, body: Bytes) -> Response {
    handler_handshake_post_impl(State(state), body).await
}

async fn handler_handshake_post_impl(State(state): State_, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<HandshakeRequest>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    match sister::receive_handshake(state.backend.as_ref(), &state.cipher, &req.instance_name, &req.secret).await {
        Ok(ok) => json_response(StatusCode::OK, &serde_json::json!({ "ok": ok })),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// `POST /_federation/sister/ban-event` — bearer-authenticated with the
/// shared sister secret; the caller never states its own name, only
/// proves it holds the secret (`sister::sister_by_secret`).
#[cfg(any(test, feature = "test-util"))]
pub async fn handler_ban_event_post(State(state): State_, headers: HeaderMap, body: Bytes) -> Response {
    handler_ban_event_post_impl(State(state), headers, body).await
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) async fn handler_ban_event_post(State(state): State_, headers: HeaderMap, body: Bytes) -> Response {
    handler_ban_event_post_impl(State(state), headers, body).await
}

async fn handler_ban_event_post_impl(State(state): State_, headers: HeaderMap, body: Bytes) -> Response {
    let Some(secret) = bearer_token(&headers) else {
        return json_error(StatusCode::UNAUTHORIZED, "missing bearer token");
    };
    let sister_peer = match sister::sister_by_secret(state.backend.as_ref(), &state.cipher, &secret).await {
        Ok(Some(peer)) => peer,
        _ => return json_error(StatusCode::UNAUTHORIZED, "unknown sister credential"),
    };
    let Ok(event) = serde_json::from_slice::<BanEventWire>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    match sister::apply_ban_event(state.backend.as_ref(), &state.cipher, &sister_peer.name, &event).await {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// `GET /_federation/sister/bans` — full current ban list (not a delta),
/// for the periodic reconciliation pull (`sister::reconcile_with_sister`).
/// Same bearer authentication as the ban-event push.
#[cfg(any(test, feature = "test-util"))]
pub async fn handler_bans_get(State(state): State_, headers: HeaderMap) -> Response {
    handler_bans_get_impl(State(state), headers).await
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) async fn handler_bans_get(State(state): State_, headers: HeaderMap) -> Response {
    handler_bans_get_impl(State(state), headers).await
}

async fn handler_bans_get_impl(State(state): State_, headers: HeaderMap) -> Response {
    let Some(secret) = bearer_token(&headers) else {
        return json_error(StatusCode::UNAUTHORIZED, "missing bearer token");
    };
    if sister::sister_by_secret(state.backend.as_ref(), &state.cipher, &secret)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return json_error(StatusCode::UNAUTHORIZED, "unknown sister credential");
    }
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default();
    let out: Vec<SisterBanEntry> = peers
        .into_iter()
        .filter(|p| p.banned)
        .map(|p| SisterBanEntry {
            peer_name: p.name,
            git_base_url: p.git_base_url,
            ban_reason: p.ban_reason.unwrap_or_default(),
            banned_by: p.banned_by,
            banned_at: p.banned_at,
        })
        .collect();
    json_response(StatusCode::OK, &out)
}
