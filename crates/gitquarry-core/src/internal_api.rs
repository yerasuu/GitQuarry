//! Loopback-only `/_internal/*` API — what a `gitquarry-artifacts` process
//! calls into core for (`docs/artifacts-concept.md`). Registered only when
//! `[artifacts]` is present in `gitquarry.toml` (`config::ArtifactsSection`);
//! bound to `internal_bind` (an operator's responsibility to keep that
//! loopback-only) and, defense in depth, every request must also carry the
//! shared secret in the `X-Internal-Token` header matching `internal_token`
//! — the same secret `gitquarry-artifacts.toml` is manually configured with.
//!
//! `gitquarry-artifacts` never receives a [`crate::auth::entities::Permission`]
//! or [`crate::auth::entities::User`] over this API: every access decision
//! is made here, in core, the source of truth, never cached or re-derived on
//! the artifacts side.

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::Permission;
use crate::storage::entities::OwnerRef;
use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use serde::{Deserialize, Serialize};
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
        .route(
            "/_internal/artifacts/namespace-permission",
            post(handler_artifacts_namespace_permission),
        )
        .with_state(ctx)
}

fn json_error(status: StatusCode, message: &str) -> axum::response::Response {
    gitquarry_wire::ipc::json_error(status, message)
}

#[derive(Deserialize)]
struct NamespacePermissionRequest {
    username: String,
    owner: OwnerRef,
    require: Permission,
}

#[derive(Serialize)]
struct NamespacePermissionResponse {
    ok: bool,
}

/// `POST /_internal/artifacts/namespace-permission` — `gitquarry-artifacts`'s
/// own publish-authorization check: does `username` hold at least `require`
/// over `owner` (a user's own namespace, or a group)? See
/// `access::effective_namespace_permission_for_user`'s own doc comment for
/// the actual decision.
async fn handler_artifacts_namespace_permission(
    State(ctx): State<Arc<AppContext>>,
    axum::Json(body): axum::Json<NamespacePermissionRequest>,
) -> axum::response::Response {
    let Ok((Some(user), _etag)) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &body.username).await else {
        return json_error(StatusCode::NOT_FOUND, "no such user");
    };
    let effective =
        match access::effective_namespace_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &user, &body.owner)
            .await
        {
            Ok(effective) => effective,
            Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
        };
    let ok = effective.is_some_and(|p| p >= body.require);
    axum::Json(NamespacePermissionResponse { ok }).into_response()
}
