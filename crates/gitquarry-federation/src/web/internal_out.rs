//! `/_internal-out/*` — the other direction of the IPC boundary: what
//! `gitquarry-core` calls *into* federation for (section 3A of
//! `docs/federation-binary-split-concept.md`). Same loopback +
//! `internal_token` protection as everything else on this router (see
//! `web::run_server`'s own doc comment) — core is the only caller these
//! were ever designed for.

use crate::access;
use crate::domain::fork::{self, DestOwnerKind, RequestFork};
use crate::entities::PeerAccessLevel;
use crate::state::State_;
use crate::web::session::forwarded_user_uid;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gitquarry_wire::{RepoSummary, Uid};
use serde::{Deserialize, Serialize};

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

#[derive(Serialize)]
struct PeerLookup {
    web_base_url: String,
    git_base_url: String,
    access_level: PeerAccessLevel,
    banned: bool,
}

/// Feeds `/_remote/{peer}/{*repo}` on core — replaces core reading
/// `.conf/peers` locally now that `Peer` is federation-owned storage. Also
/// what a fork/PR-proxy request site checks `access_level`/`banned`
/// against before proceeding (section 6) — core's own browse handler must
/// check `banned` here too, same as `domain::fork`/`web::remote_pulls` do,
/// so a banned peer is cut off from browsing as well, not just fork/PR-proxy.
pub(crate) async fn handler_peer_lookup(State(state): State_, Path(name): Path<String>) -> Response {
    match access::get_peer(state.backend.as_ref(), &state.cipher, &name).await {
        Ok(Some(peer)) => json_response(
            StatusCode::OK,
            &PeerLookup {
                web_base_url: peer.web_base_url,
                git_base_url: peer.git_base_url,
                access_level: peer.access_level,
                banned: peer.banned,
            },
        ),
        _ => (StatusCode::NOT_FOUND, "no such peer").into_response(),
    }
}

#[derive(Serialize)]
struct PeerInfo {
    name: String,
    web_base_url: String,
    git_base_url: String,
    linked_at: String,
}

/// Feeds the `/` index's federated section on core — replaces
/// `AppContext::peer_cache`. Excludes banned peers: cut off means cut off
/// everywhere, not just fork/PR-proxy.
pub(crate) async fn handler_peers_list(State(state): State_) -> Response {
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default();
    let out: Vec<PeerInfo> = peers
        .into_iter()
        .filter(|p| !p.banned)
        .map(|p| PeerInfo {
            name: p.name,
            web_base_url: p.web_base_url,
            git_base_url: p.git_base_url,
            linked_at: p.linked_at,
        })
        .collect();
    json_response(StatusCode::OK, &out)
}

#[derive(Serialize)]
struct PeerRepos {
    peer_name: String,
    repos: Vec<RepoSummary>,
}

/// Feeds the `/` index's/search's federated section on core — replaces
/// the data `run_periodic_refresh` used to cache. Live fan-out fetch for
/// now, one round-trip per linked peer per call: the gossip layer (section
/// 5) replaces this with a cache refreshed on its own tick, same as the
/// single-binary predecessor's periodic refresh did, but this endpoint's
/// shape doesn't change either way — only what backs it does.
pub(crate) async fn handler_peer_repos(State(state): State_) -> Response {
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .unwrap_or_default();
    let mut set = tokio::task::JoinSet::new();
    for peer in peers.into_iter().filter(|p| !p.banned) {
        set.spawn(async move {
            let repos = crate::instance_client::fetch_repos(&peer.web_base_url)
                .await
                .unwrap_or_default();
            PeerRepos {
                peer_name: peer.name,
                repos,
            }
        });
    }
    let mut out = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(entry) = result {
            out.push(entry);
        }
    }
    out.sort_by(|a, b| a.peer_name.cmp(&b.peer_name));
    json_response(StatusCode::OK, &out)
}

#[derive(Serialize)]
struct CredentialLookup {
    secret: String,
}

#[derive(Deserialize)]
struct ForkRequest {
    peer_name: String,
    source_repo_name: String,
    dest_full_name: String,
    /// `"user"` or `"group"` — matches `DestOwnerKind`'s own wire repr on
    /// the `core_client::ForkFromRemote` side of this same round trip.
    dest_owner_kind: String,
    dest_owner_uid: Uid,
    #[serde(default)]
    description: Option<String>,
    acting_username: String,
}

/// `POST /_internal-out/fork` — core's own trigger for federation layer
/// 2's fork-from-peer (`webui::fork::handler_fork_post`'s peer branch).
/// Core already authenticated the session and resolved the destination
/// owner itself (it owns `User`/`Group`, federation doesn't); this call
/// only needs [`fork::request_fork`] to re-validate the *source* peer link
/// (not banned, at least `BrowseAndFork`) before forwarding to core's own
/// `POST /_internal/repos/fork-from-remote`, which does the actual git
/// mechanic and re-verifies the destination itself server-side.
pub(crate) async fn handler_fork_post(State(state): State_, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<ForkRequest>(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response();
    };
    let dest_owner_kind = match req.dest_owner_kind.as_str() {
        "user" => DestOwnerKind::User,
        "group" => DestOwnerKind::Group,
        other => return (StatusCode::BAD_REQUEST, format!("unknown dest_owner_kind: {other}")).into_response(),
    };
    match fork::request_fork(
        state.backend.as_ref(),
        &state.cipher,
        &state.core,
        RequestFork {
            peer_name: &req.peer_name,
            source_repo_name: &req.source_repo_name,
            dest_full_name: &req.dest_full_name,
            dest_owner_kind,
            dest_owner_uid: &req.dest_owner_uid,
            description: req.description,
            acting_username: &req.acting_username,
        },
    )
    .await
    {
        Ok(outcome) => json_response(StatusCode::OK, &outcome),
        Err(e) => json_response(
            StatusCode::OK,
            &serde_json::json!({ "ok": false, "reason": e.to_string() }),
        ),
    }
}

/// Feeds core's `/_remote/{peer}/{*repo}?view=pulls` tab: the viewer's own
/// linked [`crate::access::PeerCredential`] secret for `peer_name`, so core
/// can call that peer's public `/_api/pulls*` directly with it (the read
/// side of federation layer 4's PR-proxy stays entirely core-side — see
/// `web::remote_pulls`'s own doc comment on why only the *write* needs
/// federation). `X-Gitquarry-User-Uid` is core's own forwarded session
/// header, same trust boundary every other `/_internal-out/*` call and
/// every session-gated proxy route already relies on.
pub(crate) async fn handler_credential_lookup(
    State(state): State_,
    Path(peer_name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user_uid) = forwarded_user_uid(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing X-Gitquarry-User-Uid").into_response();
    };
    match access::get_peer_credential(state.backend.as_ref(), &state.cipher, &user_uid, &peer_name).await {
        Ok(Some(credential)) => json_response(
            StatusCode::OK,
            &CredentialLookup {
                secret: credential.secret,
            },
        ),
        _ => (StatusCode::NOT_FOUND, "no linked credential for this peer").into_response(),
    }
}

#[derive(Deserialize)]
struct TokenResolveRequest {
    secret: String,
    repo_uid: Uid,
}

#[derive(Serialize)]
struct TokenResolveResponse {
    ok: bool,
    username: Option<String>,
    /// `false` = read-only, core caps at `Permission::Reporter` regardless
    /// of the owner's real tier. `true` = read+write, core caps only at the
    /// owner's own real permission — never higher. Mirrors
    /// `entities::AccessToken::write`'s own doc comment; federation never
    /// computes a `Permission` itself, core does that from `username` alone.
    write: Option<bool>,
}

const NOT_RESOLVED: TokenResolveResponse = TokenResolveResponse {
    ok: false,
    username: None,
    write: None,
};

/// `POST /_internal-out/tokens/resolve {secret, repo_uid}` — the fine-
/// grained-token-as-git-credential path `git::http` (core) needs
/// (`docs/federation-binary-split-concept.md` section 3B's "Nota de
/// compatibilidad futura", now built): resolves a bearer secret to its
/// owning username, but only if `repo_uid` is one of the exact repos the
/// token lists — "fine-grained" means never "every repo" (see
/// `access::resolve_by_token`'s own doc comment on why this never returns a
/// full `User`/`Permission`, only a username for core to re-derive both
/// from itself).
pub(crate) async fn handler_tokens_resolve_post(State(state): State_, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<TokenResolveRequest>(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response();
    };
    let Ok(Some((username, token))) =
        access::resolve_by_token(state.backend.as_ref(), &state.cipher, &req.secret).await
    else {
        return json_response(StatusCode::OK, &NOT_RESOLVED);
    };
    if !token.repos.contains(&req.repo_uid) {
        return json_response(StatusCode::OK, &NOT_RESOLVED);
    }
    access::touch_token_last_used(state.backend.as_ref(), &state.cipher, &token).await;
    json_response(
        StatusCode::OK,
        &TokenResolveResponse {
            ok: true,
            username: Some(username),
            write: Some(token.write),
        },
    )
}
