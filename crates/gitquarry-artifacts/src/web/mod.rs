//! This process's own loopback HTTP server — `listen` in
//! `gitquarry-artifacts.toml`. Phase 1 skeleton only (`docs/artifacts-concept.md`):
//! no `/_packages/*` npm/raw routes yet (Phase 2/3) — just a guarded health
//! check to prove the loopback IPC leg (core ↔ artifacts) actually works
//! end to end before any real feature lands on top of it.

use crate::state::ArtifactsState;
use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;
use std::sync::Arc;

async fn handler_health() -> &'static str {
    "ok"
}

pub fn router(state: Arc<ArtifactsState>) -> Router {
    Router::new()
        .route("/_internal/health", get(handler_health))
        .with_state(state)
}

pub async fn run_server(bind: &str, state: Arc<ArtifactsState>, internal_token: String) -> Result<()> {
    let app = router(state).layer(axum::middleware::from_fn(move |req, next| {
        let token = internal_token.clone();
        async move { gitquarry_wire::ipc::require_internal_token(&token, req, next).await }
    }));
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding gitquarry-artifacts's own listener to {bind}"))?;
    axum::serve(listener, app)
        .await
        .context("gitquarry-artifacts's web server failed")
}
