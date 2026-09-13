//! Loopback-only `/_internal/*` API — what a `gitquarry-ci` process calls
//! into core for (`docs/ci-cd-concept.md`). Registered only when `[ci]` is
//! present in `gitquarry.toml` (`config::CiSection`); bound to
//! `internal_bind` (an operator's responsibility to keep that loopback-only)
//! and, defense in depth, every request must also carry the shared secret
//! in the `X-Internal-Token` header matching `internal_token` — the same
//! secret `gitquarry-ci.toml` is manually configured with.
//!
//! `gitquarry-ci` never receives a [`crate::auth::entities::Permission`] or
//! [`crate::auth::entities::User`] over this API: every access decision is
//! made here, in core, the source of truth, never cached or re-derived on
//! the ci side.

use crate::app::AppContext;
use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

pub async fn run_server(bind: &str, ctx: Arc<AppContext>, internal_token: String) -> Result<()> {
    let app = router(ctx).layer(axum::middleware::from_fn(move |req, next| {
        let token = internal_token.clone();
        async move { gitquarry_wire::ipc::require_internal_token(&token, req, next).await }
    }));
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding the internal API to {bind}"))?;
    axum::serve(listener, app)
        .await
        .context("the internal API server failed")
}

fn router(ctx: Arc<AppContext>) -> Router {
    Router::new()
        .route("/_internal/ci/repo", get(handler_ci_repo_info))
        .with_state(ctx)
}

fn json_error(status: StatusCode, message: &str) -> axum::response::Response {
    gitquarry_wire::ipc::json_error(status, message)
}

#[derive(Serialize)]
struct CiRepoInfoResponse {
    full_name: String,
    default_branch: Option<String>,
    head_sha: Option<String>,
    ci_deploy_gate_disabled: bool,
}

/// `GET /_internal/ci/repo?uid=` — enough of a repo's own identity for a
/// pipeline run to check something out: its display name, current default
/// branch and that branch's tip sha (`Manifest::head`/`Manifest::refs`), and
/// whether `RepoConfig::ci_deploy_gate_disabled` is set. Deliberately
/// visibility-unfiltered, same posture `handler_authorize`'s own doc comment
/// establishes elsewhere in this codebase: an internal trusted caller, not a
/// public listing — `gitquarry-ci` runs pipelines for private repos too.
async fn handler_ci_repo_info(
    State(ctx): State<Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let Some(uid) = params.get("uid") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?uid=");
    };
    let uid = crate::storage::uid::Uid::from(uid.clone());
    let remote = ctx.remote_for_uid(&uid);
    let Ok((config, etag)) = remote.get_repo_config().await else {
        return json_error(StatusCode::NOT_FOUND, "no such repo");
    };
    if etag.is_none() {
        return json_error(StatusCode::NOT_FOUND, "no such repo");
    }
    let Ok((manifest, _etag)) = remote.get_manifest().await else {
        return json_error(StatusCode::BAD_REQUEST, "couldn't read the repo's manifest");
    };
    let default_branch = manifest
        .head
        .as_deref()
        .and_then(|head| head.strip_prefix("refs/heads/"))
        .map(str::to_string);
    let head_sha = manifest
        .head
        .as_deref()
        .and_then(|head| manifest.refs.get(head))
        .cloned();
    axum::Json(CiRepoInfoResponse {
        full_name: config.full_name,
        default_branch,
        head_sha,
        ci_deploy_gate_disabled: config.ci_deploy_gate_disabled,
    })
    .into_response()
}
