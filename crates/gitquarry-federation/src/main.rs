//! `gitquarry-federation`: loads `gitquarry-federation.toml`, bootstraps
//! its own storage backend + cipher against the *same* bucket
//! `gitquarry-core` uses (different code, same passphrase/namespace — see
//! `docs/federation-binary-split-concept.md` section 4), and serves its
//! loopback API (`web::run_server`) until shutdown.

use anyhow::{Context, Result};
use gitquarry_federation::config::Config;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::gossip::{self, GossipCandidateWire, GossipTable};
use gitquarry_federation::rate_limit::PeerRateLimiter;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("gitquarry-federation: fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let config_path = Config::resolve_path()?;
    let config = Config::load_from(&config_path).with_context(|| format!("loading {}", config_path.display()))?;

    let backend = s3db::build_backend(&config.backend);
    let cipher = s3db::bootstrap::get_or_create_cipher(backend.as_ref(), &config.passphrase).await?;

    let self_announce = match (&config.own_web_base_url, &config.own_git_base_url) {
        (Some(web), Some(git)) => Some(GossipCandidateWire {
            name: gitquarry_federation::state::bucket_label(&config.backend).to_string(),
            web_base_url: web.clone(),
            git_base_url: git.clone(),
        }),
        _ => {
            if !config.seed_peers.is_empty() {
                eprintln!(
                    "gitquarry-federation: warning: seed_peers is set but own_web_base_url/own_git_base_url \
                     aren't both configured — this instance will answer gossip but never initiate it"
                );
            }
            None
        }
    };

    let state = Arc::new(FederationState {
        backend,
        cipher: Arc::new(cipher),
        core: CoreClient::new(config.core_internal_url.clone(), config.internal_token.clone()),
        backend_config: config.backend.clone(),
        gossip: Arc::new(GossipTable::default()),
        self_announce,
        rate_limiter: Arc::new(PeerRateLimiter::default()),
        rate_limit_public_max: config.rate_limit_public_max,
        rate_limit_public_window: Duration::from_secs(config.rate_limit_public_window_secs),
        rate_limit_token_max: config.rate_limit_token_max,
        rate_limit_token_window: Duration::from_secs(config.rate_limit_token_window_secs),
        rate_limit_peer_override_by_ip: config.rate_limit_peer_override_by_ip,
    });

    if let Some(self_info) = state.self_announce.clone() {
        gossip::bootstrap_seeds(&state.gossip, &config.seed_peers, &self_info).await;
    }
    tokio::spawn(gossip::run_periodic_tick(
        Arc::clone(&state),
        config.gossip_interval_secs,
        config.auto_link_min_sources,
        config.auto_link_min_age_secs,
    ));
    tokio::spawn(gitquarry_federation::sister::run_periodic_reconcile_tick(
        Arc::clone(&state.backend),
        Arc::clone(&state.cipher),
        config.sister_reconcile_interval_secs,
    ));

    web::run_server(&config.listen, state, config.internal_token).await
}
