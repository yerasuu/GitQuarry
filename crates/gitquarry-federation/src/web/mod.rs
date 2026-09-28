//! This process's own loopback HTTP server — `listen` in
//! `gitquarry-federation.toml`. Every route here is reached exactly one
//! way: through `gitquarry-core`'s proxy (`docs/federation-binary-split-concept.md`
//! section 3A/3C), which is itself the only thing that ever calls this
//! bind address. Accordingly the *whole* router (not a subset) requires
//! the shared `X-Internal-Token` secret — "ambos lados protegen las
//! llamadas loopback con el secreto compartido" is section 3's own
//! framing, and core is the only legitimate caller of any route here,
//! public-API-shaped or not.

#[cfg(any(test, feature = "test-util"))]
pub mod api;
#[cfg(not(any(test, feature = "test-util")))]
mod api;
mod api_pulls;
mod gossip;
mod internal_out;
#[cfg(any(test, feature = "test-util"))]
pub mod peers;
#[cfg(not(any(test, feature = "test-util")))]
mod peers;
#[cfg(any(test, feature = "test-util"))]
pub mod remote_pulls;
#[cfg(not(any(test, feature = "test-util")))]
mod remote_pulls;
pub mod session;
#[cfg(any(test, feature = "test-util"))]
pub mod sister;
#[cfg(not(any(test, feature = "test-util")))]
mod sister;
#[cfg(any(test, feature = "test-util"))]
pub mod tokens;
#[cfg(not(any(test, feature = "test-util")))]
mod tokens;

#[cfg(any(test, feature = "test-util"))]
pub mod rate_limit_middleware;
#[cfg(not(any(test, feature = "test-util")))]
mod rate_limit_middleware;

use crate::state::FederationState;
use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;
use std::sync::Arc;

/// `/_api/*` and `/_federation/gossip/*` — genuinely externally-reached
/// (via core's proxy, or peer-to-peer for gossip), so this is the one
/// group `rate_limit_middleware` actually protects (section 6). Split from
/// [`other_routes`] so the rate-limit layer applies to exactly this
/// surface, not to the session-gated or `/_internal-out/*` groups, which
/// have their own gates and are never hit by an arbitrary caller.
fn public_routes(state: Arc<FederationState>) -> Router {
    Router::new()
        .route("/_api/instance", get(api::handler_instance_get))
        .route("/_api/repos", get(api::handler_repos_get))
        .route("/_api/repos/{*name}", get(api::handler_repo_get))
        .route(
            "/_api/pulls",
            get(api_pulls::handler_list_get).post(api_pulls::handler_create_post),
        )
        .route("/_api/pulls/detail", get(api_pulls::handler_detail_get))
        .route(
            "/_api/pulls/comment",
            axum::routing::post(api_pulls::handler_comment_post),
        )
        .route("/_api/pulls/merge", axum::routing::post(api_pulls::handler_merge_post))
        .route("/_api/banned-peers", get(api::handler_banned_peers_get))
        .route(
            "/_federation/gossip/announce",
            axum::routing::post(gossip::handler_announce),
        )
        .route("/_federation/gossip/peers", get(gossip::handler_peers))
        .route(
            "/_federation/sister/handshake",
            axum::routing::post(sister::handler_handshake_post),
        )
        .route(
            "/_federation/sister/ban-event",
            axum::routing::post(sister::handler_ban_event_post),
        )
        .route("/_federation/sister/bans", get(sister::handler_bans_get))
        .layer(axum::middleware::from_fn(rate_limit_middleware::layer(Arc::clone(
            &state,
        ))))
        .with_state(state)
}

fn other_routes(state: Arc<FederationState>) -> Router {
    Router::new()
        .route("/_peers", get(peers::handler_peers_get).post(peers::handler_peers_post))
        .route(
            "/_remote_pulls/comment",
            axum::routing::post(remote_pulls::handler_comment_post),
        )
        .route(
            "/_account/tokens",
            get(tokens::handler_tokens_get).post(tokens::handler_tokens_post),
        )
        .route("/_internal-out/peer/{name}", get(internal_out::handler_peer_lookup))
        .route("/_internal-out/peers", get(internal_out::handler_peers_list))
        .route("/_internal-out/peer-repos", get(internal_out::handler_peer_repos))
        .route(
            "/_internal-out/credential/{peer_name}",
            get(internal_out::handler_credential_lookup),
        )
        .route(
            "/_internal-out/fork",
            axum::routing::post(internal_out::handler_fork_post),
        )
        .route(
            "/_internal-out/tokens/resolve",
            axum::routing::post(internal_out::handler_tokens_resolve_post),
        )
        .with_state(state)
}

pub fn router(state: Arc<FederationState>) -> Router {
    public_routes(Arc::clone(&state)).merge(other_routes(state))
}

pub async fn run_server(bind: &str, state: Arc<FederationState>, internal_token: String) -> Result<()> {
    let app = router(state).layer(axum::middleware::from_fn(move |req, next| {
        let token = internal_token.clone();
        async move { gitquarry_wire::ipc::require_internal_token(&token, req, next).await }
    }));
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding gitquarry-federation's own listener to {bind}"))?;
    axum::serve(listener, app)
        .await
        .context("gitquarry-federation's web server failed")
}
