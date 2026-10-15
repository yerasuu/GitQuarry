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
/// `sister_requested` on for `instance_name` *and* the instance at that
/// peer's registered URL vouches for the secret (`sister::receive_handshake`'s
/// own doc comment). Needs `own_web_base_url`/`own_git_base_url` set, since
/// the dial-back has to tell the peer who is asking.
pub(crate) async fn handler_handshake_post(State(state): State_, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<HandshakeRequest>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let Some(own) = &state.self_announce else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "this instance has no own_web_base_url/own_git_base_url configured, so it can't verify a sister handshake",
        );
    };
    match sister::receive_handshake(
        state.backend.as_ref(),
        &state.cipher,
        &own.name,
        &req.instance_name,
        &req.secret,
    )
    .await
    {
        Ok(ok) => json_response(StatusCode::OK, &serde_json::json!({ "ok": ok })),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &format!("{e:#}")),
    }
}

/// `POST /_federation/sister/verify` — the dial-back a peer makes while
/// handling *our* handshake: `instance_name` is that peer's name as this
/// instance knows it, `secret` the one it was offered. Answers only
/// yes/no, so it reveals nothing to a caller who doesn't already hold the
/// secret.
pub(crate) async fn handler_verify_post(State(state): State_, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<HandshakeRequest>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    match sister::verify_staged_secret(state.backend.as_ref(), &state.cipher, &req.instance_name, &req.secret).await {
        Ok(ok) => json_response(StatusCode::OK, &serde_json::json!({ "ok": ok })),
        Err(e) => {
            tracing::error!(target: "gitquarry_federation::web::sister", error = %format!("{e:#}"), "sister verify failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "couldn't check the secret")
        }
    }
}

/// `POST /_federation/sister/ban-event` — bearer-authenticated with the
/// shared sister secret; the caller never states its own name, only
/// proves it holds the secret (`sister::sister_by_secret`).
pub(crate) async fn handler_ban_event_post(State(state): State_, headers: HeaderMap, body: Bytes) -> Response {
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
pub(crate) async fn handler_bans_get(State(state): State_, headers: HeaderMap) -> Response {
    let Some(secret) = bearer_token(&headers) else {
        return json_error(StatusCode::UNAUTHORIZED, "missing bearer token");
    };
    let Ok(Some(caller)) = sister::sister_by_secret(state.backend.as_ref(), &state.cipher, &secret).await else {
        return json_error(StatusCode::UNAUTHORIZED, "unknown sister credential");
    };
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default();
    // Bans learned from the caller are its own decisions; handing them back
    // would let a ban it has since lifted (and whose unban push got lost)
    // come straight back to it on its next pull.
    let out: Vec<SisterBanEntry> = peers
        .into_iter()
        .filter(|p| p.banned && p.synced_from_sister.as_deref() != Some(caller.name.as_str()))
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
