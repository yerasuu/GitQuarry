//! This process's own loopback HTTP server — `listen` in
//! `gitquarry-ci.toml`. Phase 1 skeleton only (`docs/ci-cd-concept.md`):
//! no pipeline-trigger endpoint yet, since core has no event-bus consumer
//! registered for this crate yet either — just a guarded health check to
//! prove the loopback IPC leg (core ↔ ci, both directions) actually works
//! end to end before any real feature lands on top of it.

use crate::graph;
use crate::state::CiState;
use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use gitquarry_wire::JobGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

async fn handler_health() -> &'static str {
    "ok"
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    #[derive(Serialize)]
    struct ErrorBody {
        error: String,
    }
    (status, Json(ErrorBody { error: message.into() })).into_response()
}

#[derive(Deserialize)]
struct IngestGraphRequest {
    repo_uid: String,
    sha: String,
    graph: JobGraph,
}

/// `POST /_internal/graph` — what a pipeline-trigger consumer calls once
/// it has a `JobGraph` dumped by either SDK: validates it and stores one
/// `JobRun` per job, applying the deploy-approval gate.
async fn handler_ingest_graph(State(state): State<Arc<CiState>>, Json(req): Json<IngestGraphRequest>) -> Response {
    match graph::ingest_graph(
        &state.backend,
        &state.cipher,
        &state.core,
        &req.repo_uid,
        &req.sha,
        req.graph,
    )
    .await
    {
        Ok(runs) => Json(runs).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// `GET /_internal/runs/pending?repo_uid=` — deploy jobs awaiting
/// maintainer approval for one repo, what `gitquarry-webui`'s own CI
/// approval page proxies to render.
async fn handler_pending_runs(
    State(state): State<Arc<CiState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(repo_uid) = params.get("repo_uid") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?repo_uid=");
    };
    match graph::list_pending_approvals(&state.backend, &state.cipher, repo_uid).await {
        Ok(runs) => Json(runs).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct ApproveRunRequest {
    repo_uid: String,
    run_id: String,
}

/// `POST /_internal/runs/approve` — a maintainer approving one pending
/// `deploy` run. Authorization (is this caller actually a maintainer of
/// `repo_uid`?) is core's job, already checked before `gitquarry-webui`
/// ever proxies this call — see that module's own doc comment.
async fn handler_approve_run(State(state): State<Arc<CiState>>, Json(req): Json<ApproveRunRequest>) -> Response {
    match graph::approve_run(&state.backend, &state.cipher, &req.repo_uid, &req.run_id).await {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => json_error(StatusCode::NOT_FOUND, "no such run"),
        Err(e) => json_error(StatusCode::CONFLICT, e.to_string()),
    }
}

pub fn router(state: Arc<CiState>) -> Router {
    Router::new()
        .route("/_internal/health", get(handler_health))
        .route("/_internal/graph", post(handler_ingest_graph))
        .route("/_internal/runs/pending", get(handler_pending_runs))
        .route("/_internal/runs/approve", post(handler_approve_run))
        .with_state(state)
}

pub async fn run_server(bind: &str, state: Arc<CiState>, internal_token: String) -> Result<()> {
    let app = router(state).layer(axum::middleware::from_fn(move |req, next| {
        let token = internal_token.clone();
        async move { gitquarry_wire::ipc::require_internal_token(&token, req, next).await }
    }));
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding gitquarry-ci's own listener to {bind}"))?;
    axum::serve(listener, app)
        .await
        .context("gitquarry-ci's web server failed")
}
