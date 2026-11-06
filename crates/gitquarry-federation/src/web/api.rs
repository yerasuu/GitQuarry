//! Read-only JSON API for another gitquarry instance to discover this
//! one's public repos over HTTP — federation layer 1. No authentication
//! and no gate for the plain, unauthenticated case: exactly as exposed as
//! core's own `/` index already is to an anonymous visitor.
//!
//! Extended with an optional `Authorization: Bearer <token>` — a valid
//! fine-grained [`crate::entities::AccessToken`] adds whatever
//! `private`/`limited` repos it's scoped to (and its owner still genuinely
//! has read access to, checked live via `POST /_internal/authorize` on
//! core) on top of the same public list every caller already sees. A
//! missing, malformed, or invalid token is never an error here, just
//! silently no extra repos.

use crate::access;
use crate::state::{FederationState, State_, bucket_label};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gitquarry_wire::{InstanceInfo, RepoSummary};
use serde::Serialize;

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

#[derive(Serialize)]
struct ApiError<'a> {
    error: &'a str,
}

fn json_error(status: StatusCode, message: &str) -> Response {
    json_response(status, &ApiError { error: message })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    Some(value.strip_prefix("Bearer ")?.trim().to_string())
}

/// Every ban this instance has ever issued, currently in effect — always
/// recomputed live from the `Peer` collection (`count where banned ==
/// true`), never a stored counter that could drift out of sync with the
/// actual ban list (section 6, "Accountability contra abuso de bans").
/// Combined with the full detail at `GET /_api/banned-peers`, lets another
/// instance's admin spot a burst of bans at a glance during routine
/// discovery, without pulling the whole list every time.
///
/// Once sister-instance ban sync exists (section 7), this must only count
/// bans this instance originated itself, not ones it merely inherited from
/// a sister — otherwise two sisters' identical, mirrored ban lists would
/// each report the other's decisions as their own, inflating both.
/// Only bans this instance itself issued — one synced from a sister
/// (`entities::Peer::synced_from_sister`'s own doc comment) is excluded so
/// two sisters' identical, mirrored ban lists don't each inflate the
/// other's accountability count for what is really one decision.
async fn count_bans_issued(state: &FederationState) -> u64 {
    access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default()
        .iter()
        .filter(|p| p.banned && p.synced_from_sister.is_none())
        .count() as u64
}

pub(crate) async fn handler_instance_get(State(state): State_) -> Response {
    let body = InstanceInfo {
        name: bucket_label(&state.backend_config).to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        total_bans_issued: count_bans_issued(&state).await,
    };
    json_response(StatusCode::OK, &body)
}

#[derive(Serialize)]
struct BannedPeer {
    peer_name: String,
    git_base_url: String,
    ban_reason: String,
    banned_at: Option<String>,
    banned_by: String,
}

/// `GET /_api/banned-peers` — no auth, deliberately (section 6:
/// "Visibilidad publica... el motivo de ban siempre es publico, no
/// solo-admin"): every `Peer` this instance currently has banned, with
/// both the reason and who banned it, so another instance's admin can
/// judge for themselves whether to trust this one — the same transparency
/// mechanism `total_bans_issued` on `/_api/instance` complements.
pub(crate) async fn handler_banned_peers_get(State(state): State_) -> Response {
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default();
    let out: Vec<BannedPeer> = peers
        .into_iter()
        .filter(|p| p.banned)
        .map(|p| BannedPeer {
            peer_name: p.name,
            git_base_url: p.git_base_url,
            ban_reason: p.ban_reason.unwrap_or_default(),
            banned_at: p.banned_at,
            banned_by: p.banned_by,
        })
        .collect();
    json_response(StatusCode::OK, &out)
}

pub(crate) async fn handler_repos_get(State(state): State_, headers: HeaderMap) -> Response {
    let repos = visible_repos(&state, &headers).await;
    json_response(StatusCode::OK, &repos)
}

pub(crate) async fn handler_repo_get(State(state): State_, Path(name): Path<String>, headers: HeaderMap) -> Response {
    let repos = visible_repos(&state, &headers).await;
    match repos.into_iter().find(|r| r.name == name) {
        Some(repo) => json_response(StatusCode::OK, &repo),
        None => json_error(StatusCode::NOT_FOUND, "no such repo, or no access"),
    }
}

async fn visible_repos(state: &FederationState, headers: &HeaderMap) -> Vec<RepoSummary> {
    let mut repos = state.core.public_repos().await.unwrap_or_default();
    for extra in token_repos(state, headers).await {
        if !repos.iter().any(|r| r.name == extra.name) {
            repos.push(extra);
        }
    }
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    repos
}

/// The `private`/`limited` repos a `Bearer` token grants read access to —
/// empty for a missing/malformed header or an unknown/revoked/expired
/// secret. `Reporter` is the read floor for a listing, same as core's own
/// `git::http`/web-UI private-repo read gate — a read-only *and* a
/// read+write token both clear it, so `token.write` never matters here
/// (only for the higher-tier PR-proxy write actions — see `web::api_pulls`).
async fn token_repos(state: &FederationState, headers: &HeaderMap) -> Vec<RepoSummary> {
    let Some(secret) = bearer_token(headers) else {
        return Vec::new();
    };
    let Ok(Some((owner, token))) = access::resolve_by_token(state.backend.as_ref(), &state.cipher, &secret).await
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for uid in &token.repos {
        if let Ok(Some(summary)) = state.core.authorize_and_summarize(&owner, uid, "reporter").await {
            out.push(summary);
        }
    }
    out
}
