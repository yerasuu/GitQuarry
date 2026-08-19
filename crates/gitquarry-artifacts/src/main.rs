//! `gitquarry-artifacts`: loads `gitquarry-artifacts.toml`, bootstraps its
//! own storage backend + cipher against the *same* bucket `gitquarry-core`
//! uses (different code, same passphrase/namespace — see
//! `docs/artifacts-concept.md`'s IPC design decision), and serves its
//! loopback API (`web::run_server`) until shutdown. Phase 1 skeleton: no
//! npm/raw HTTP adapters yet.

use anyhow::{Context, Result};
use gitquarry_artifacts::config::Config;
use gitquarry_artifacts::core_client::CoreClient;
use gitquarry_artifacts::state::ArtifactsState;
use gitquarry_artifacts::web;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("gitquarry-artifacts: fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let config_path = Config::resolve_path()?;
    let config = Config::load_from(&config_path).with_context(|| format!("loading {}", config_path.display()))?;

    let backend = s3db::build_backend(&config.backend);
    let cipher = s3db::bootstrap::get_or_create_cipher(backend.as_ref(), &config.passphrase).await?;

    let state = Arc::new(ArtifactsState {
        backend,
        cipher: Arc::new(cipher),
        core: CoreClient::new(config.core_internal_url.clone(), config.internal_token.clone()),
    });

    web::run_server(&config.listen, state, config.internal_token).await
}
