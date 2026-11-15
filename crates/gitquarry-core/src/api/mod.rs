//! The public JSON API under `/_api/v1` (decision #144): one prefix, bearer
//! tokens ([`auth`]), one error shape ([`error`]) and opaque cursors
//! ([`cursor`]) shared by every endpoint, so a client learns them once.
//!
//! Mounted by `gitquarry_webui::webui::router` on the same port as the web
//! UI and git-smart-HTTP. Everything under the prefix answers JSON, an
//! unknown path or method included, so a client never has to parse an HTML
//! page the UI's catch-all would otherwise render.

pub mod auth;
pub mod cursor;
pub mod error;
pub mod limits;
mod whoami;

use crate::app::AppContext;
use axum::Router;
use axum::routing::{any, get};
use error::{ApiError, ErrorCode};
use std::sync::Arc;

pub const PREFIX: &str = "/_api/v1";

/// Every `/_api/v1` route, for the caller to merge into its router and
/// give the [`AppContext`] state. `ctx` is the state the caller gives it,
/// which the rate-limit layer ([`limits`]) needs before that happens.
pub fn router(ctx: &Arc<AppContext>) -> Router<Arc<AppContext>> {
    // A per-route fallback, not the merged router's: the web UI sets its own
    // `method_not_allowed_fallback` (a redirect to `/`), which only replaces
    // the routes still on axum's default one.
    Router::new()
        .route("/_api/v1/whoami", get(whoami::handler).fallback(method_not_allowed))
        .route("/_api/v1/{*rest}", any(not_found))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(ctx),
            limits::middleware,
        ))
}

async fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no such endpoint").with_hint("endpoints live under /_api/v1")
}

async fn method_not_allowed() -> ApiError {
    ApiError::new(ErrorCode::MethodNotAllowed, "this endpoint does not take that method")
}
