//! Loopback-only `/_internal/*` API — what a `gitquarry-tracker` process
//! calls into core for (`docs/tracker-binary-split-concept.md`).
//! Registered only when `[tracker]` is present in `gitquarry.toml`
//! (`config::TrackerSection`); bound to `internal_bind` (an operator's
//! responsibility to keep that loopback-only) and, defense in depth, every
//! request must also carry the shared secret in the `X-Internal-Token`
//! header matching `internal_token` — the same secret
//! `gitquarry-tracker.toml` is manually configured with.
//!
//! Tracker never receives a [`crate::auth::entities::Permission`] or
//! [`crate::auth::entities::User`] over this API: every access decision is
//! made here, in core, the source of truth, never cached or re-derived on
//! tracker's side.

use crate::app::AppContext;
use crate::auth::access;
use crate::domain;
use crate::storage;
use crate::storage::uid::Uid;
use anyhow::{Context, Result};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
        .route("/_internal/repos/labels", get(handler_repo_labels))
        .route("/_internal/users/exists", get(handler_user_exists))
        .route("/_internal/tracker/pull-request", get(handler_tracker_pull_request))
        .route("/_internal/tracker/event", post(handler_tracker_event))
        .with_state(ctx)
}

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

fn json_error(status: StatusCode, message: &str) -> Response {
    gitquarry_wire::ipc::json_error(status, message)
}

#[derive(Serialize)]
struct RepoLabelsResponse {
    labels: Vec<String>,
}

/// `GET /_internal/repos/labels?uid=` — `gitquarry-tracker`'s label-name
/// vocabulary check at issue-edit time (`domain::issue::edit` used to read
/// `target.remote.get_repo_config().await?.labels` directly when tracker was
/// still in-process; this is that same read, now over the loopback).
async fn handler_repo_labels(
    State(ctx): State<Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(uid) = params.get("uid") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?uid=");
    };
    let Ok((config, etag)) = storage::settings::get_repo_config(ctx.backend().as_ref(), uid, &ctx.cipher).await else {
        return json_error(StatusCode::NOT_FOUND, "no such repo");
    };
    if etag.is_none() {
        return json_error(StatusCode::NOT_FOUND, "no such repo");
    }
    json_response(
        StatusCode::OK,
        &RepoLabelsResponse {
            labels: config.labels.into_iter().map(|l| l.name).collect(),
        },
    )
}

#[derive(Serialize)]
struct UserExistsResponse {
    exists: bool,
}

/// `GET /_internal/users/exists?username=` — `gitquarry-tracker`'s
/// assignee-exists check at issue-edit time (used to be a direct
/// `access::get_user` call when tracker was still in-process). Narrower
/// than every other user-facing internal endpoint: no `Permission` check
/// needed, existence is all an assignee field validates against.
async fn handler_user_exists(
    State(ctx): State<Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(username) = params.get("username") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?username=");
    };
    let exists = matches!(
        access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await,
        Ok((Some(_), _))
    );
    json_response(StatusCode::OK, &UserExistsResponse { exists })
}

/// `GET /_internal/tracker/pull-request?uid=&number=` — `gitquarry-tracker`'s
/// own PR lookup, uid-keyed (tracker only ever has a uid, never a repo
/// name — unlike [`handler_pr_detail`]'s `?repo=` name param) and
/// visibility-unfiltered, same posture [`handler_authorize`]'s own doc
/// comment establishes: an internal trusted caller, not a public listing.
/// Feeds both the "closes #N" auto-close text parse (title+description) and
/// the board PR-card existence check.
async fn handler_tracker_pull_request(
    State(ctx): State<Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(uid) = params.get("uid") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?uid=");
    };
    let Some(number) = params.get("number").and_then(|s| s.parse::<u64>().ok()) else {
        return json_error(StatusCode::BAD_REQUEST, "missing or invalid ?number=");
    };
    let uid = Uid::from(uid.clone());
    let target = crate::app::ResolvedRepo {
        remote: ctx.remote_for_uid(&uid),
        uid,
    };
    match domain::pull_request::get(&ctx, &target, number).await {
        Ok(Some(pr)) => json_response(StatusCode::OK, &pr),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "no such pull request"),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// `POST /_internal/tracker/event` — `gitquarry-tracker`'s own outbound
/// domain events (issue opened/closed), re-emitted onto core's own
/// [`crate::events::EventBus`] so it stays the single source of truth for
/// "one event source, many consumers" (webhooks, future automation) —
/// `docs/tracker-binary-split-concept.md`.
async fn handler_tracker_event(State(ctx): State<Arc<AppContext>>, body: Bytes) -> Response {
    let Ok(event) = serde_json::from_slice::<gitquarry_wire::tracker::TrackerEvent>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let action = match event.kind {
        gitquarry_wire::tracker::TrackerEventKind::IssueOpened { .. } => crate::events::IssueAction::Opened,
        gitquarry_wire::tracker::TrackerEventKind::IssueClosed { .. } => crate::events::IssueAction::Closed,
    };
    let number = match event.kind {
        gitquarry_wire::tracker::TrackerEventKind::IssueOpened { number }
        | gitquarry_wire::tracker::TrackerEventKind::IssueClosed { number } => number,
    };
    crate::events::emit(
        &ctx,
        crate::events::RepoEvent::Issue {
            repo_uid: event.repo_uid,
            number,
            action,
        },
    );
    StatusCode::OK.into_response()
}
