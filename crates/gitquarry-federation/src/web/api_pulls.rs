//! Bearer-token-authenticated JSON equivalents of core's own PR forms —
//! federation layer 4's write API. Same [`crate::core_client::CoreClient`]
//! wrappers around core's `/_internal/pull-requests*` for the actual
//! storage operation; this module's whole job is turning a bearer secret
//! into "does this token cover this repo, at this tier" before ever
//! forwarding to core.
//!
//! Two checks, both required, mirroring the single-binary predecessor's
//! `effective_permission_for_token`: (1) *locally*, `token.write`'s
//! ceiling — a read-only token can never authorize a `Developer`+ action
//! regardless of what its owner could otherwise do, so this never even
//! calls core for that case; (2) via `POST /_internal/authorize` on core,
//! the owner's real, live permission. Both must pass.

use crate::access;
use crate::entities::AccessToken;
use crate::state::State_;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Reporter,
    Developer,
}

impl Tier {
    fn as_str(self) -> &'static str {
        match self {
            Tier::Reporter => "reporter",
            Tier::Developer => "developer",
        }
    }
}

/// `token.write == false` caps a token at `Reporter` (read/propose-only,
/// same floor `git::http`/the web UI require for private-repo read on
/// core); `true` lifts the local ceiling entirely — the *real* cap in that
/// case is whatever core's own `/_internal/authorize` says the owner
/// genuinely has, never anything decided here.
fn ceiling_covers(token: &AccessToken, required: Tier) -> bool {
    token.write || required <= Tier::Reporter
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    Some(value.strip_prefix("Bearer ")?.trim().to_string())
}

/// Resolves the bearer token, checks its local write ceiling, resolves
/// `repo` to a uid via core (a token scopes itself by uid, not name — see
/// `entities::AccessToken::repos`'s own doc comment), checks the uid is
/// actually listed, then asks core for the real permission decision.
/// `Err` is already a ready-to-return [`Response`].
async fn authorize(
    state: &crate::state::FederationState,
    headers: &HeaderMap,
    repo: &str,
    required: Tier,
) -> Result<String, Box<Response>> {
    let Some(secret) = bearer_token(headers) else {
        return Err(Box::new(json_error(
            StatusCode::UNAUTHORIZED,
            "missing or invalid bearer token",
        )));
    };
    let (owner, token) = match access::resolve_by_token(state.backend.as_ref(), &state.cipher, &secret).await {
        Ok(Some(resolved)) => resolved,
        Ok(None) => {
            return Err(Box::new(json_error(
                StatusCode::UNAUTHORIZED,
                "missing or invalid bearer token",
            )));
        }
        Err(e) => {
            return Err(Box::new(upstream_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "token lookup",
                &e,
            )));
        }
    };
    if !ceiling_covers(&token, required) {
        return Err(Box::new(json_error(
            StatusCode::FORBIDDEN,
            "token lacks sufficient access to this repo",
        )));
    }
    let uid = match state.core.uid_for_name(repo).await {
        Ok(Some(uid)) => uid,
        Ok(None) => return Err(Box::new(json_error(StatusCode::NOT_FOUND, "no such repo"))),
        Err(e) => return Err(Box::new(upstream_error(StatusCode::BAD_GATEWAY, "repo lookup", &e))),
    };
    if !token.repos.contains(&uid) {
        return Err(Box::new(json_error(
            StatusCode::FORBIDDEN,
            "token lacks sufficient access to this repo",
        )));
    }
    match state.core.authorize(&owner, repo, required.as_str()).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(Box::new(json_error(
                StatusCode::FORBIDDEN,
                "token lacks sufficient access to this repo",
            )));
        }
        Err(e) => return Err(Box::new(upstream_error(StatusCode::BAD_GATEWAY, "authorization", &e))),
    }
    access::touch_token_last_used(state.backend.as_ref(), &state.cipher, &token).await;
    Ok(owner)
}

/// A `Public` repo needs no token at all — same visibility bypass core's
/// own `git::http::authorize` already applies for reads. Only a
/// `private`/`limited` repo (invisible to `core.public_repo`) actually
/// requires one, and even then only `Reporter`-tier.
async fn authorize_read(
    state: &crate::state::FederationState,
    headers: &HeaderMap,
    repo: &str,
) -> Result<(), Box<Response>> {
    match state.core.public_repo(repo).await {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(e) => return Err(Box::new(upstream_error(StatusCode::BAD_GATEWAY, "repo lookup", &e))),
    }
    authorize(state, headers, repo, Tier::Reporter).await.map(|_| ())
}

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

#[derive(Serialize)]
struct ApiError<'a> {
    error: &'a str,
}

fn json_error(status: StatusCode, message: &str) -> Response {
    json_response(status, &ApiError { error: message })
}

/// A storage or core failure must never read as a credential or access
/// verdict (401/403/404) — the caller would drop a perfectly valid token
/// or conclude a repo doesn't exist. The detail stays in the log, since it
/// can name internal hosts or keys an anonymous caller has no business
/// seeing.
fn upstream_error(status: StatusCode, what: &str, error: &anyhow::Error) -> Response {
    tracing::error!(target: "gitquarry_federation::web::api_pulls", what, error = %format!("{error:#}"), "PR API dependency failed");
    json_error(status, &format!("{what} failed, try again later"))
}

pub(crate) async fn handler_list_get(
    State(state): State_,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(repo) = params.get("repo") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?repo=");
    };
    if let Err(resp) = authorize_read(&state, &headers, repo).await {
        return *resp;
    }
    match state.core.list_merge_requests(repo).await {
        Ok(prs) => json_response(StatusCode::OK, &prs),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

pub(crate) async fn handler_detail_get(
    State(state): State_,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(repo) = params.get("repo") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?repo=");
    };
    let Some(number) = params.get("number").and_then(|s| s.parse::<u64>().ok()) else {
        return json_error(StatusCode::BAD_REQUEST, "missing or invalid ?number=");
    };
    if let Err(resp) = authorize_read(&state, &headers, repo).await {
        return *resp;
    }
    match state.core.merge_request_detail(repo, number).await {
        Ok(Some(detail)) => json_response(StatusCode::OK, &detail),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "no such merge request"),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct CreatePullBody {
    repo: String,
    #[serde(default)]
    source_repo: String,
    source_branch: String,
    target_branch: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
}

pub(crate) async fn handler_create_post(State(state): State_, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(form) = serde_json::from_slice::<CreatePullBody>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let owner = match authorize(&state, &headers, &form.repo, Tier::Reporter).await {
        Ok(owner) => owner,
        Err(resp) => return *resp,
    };
    let source_repo = if form.source_repo.trim().is_empty() {
        form.repo.clone()
    } else {
        form.source_repo.trim().to_string()
    };
    // The MR exposes the source branch's commits to everyone who can see
    // the target, so the token must be able to read the source too —
    // otherwise a reporter on a public repo could lift branches out of a
    // private fork it has no access to.
    if source_repo != form.repo
        && let Err(resp) = authorize_read(&state, &headers, &source_repo).await
    {
        return *resp;
    }
    match state
        .core
        .create_merge_request(&crate::core_client::CreateMergeRequest {
            repo: &form.repo,
            source_repo: &source_repo,
            source_branch: form.source_branch.trim(),
            target_branch: form.target_branch.trim(),
            title: form.title.trim(),
            description: &form.description,
            author: &owner,
        })
        .await
    {
        Ok(pr) => json_response(StatusCode::OK, &pr),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct CommentBody {
    repo: String,
    number: u64,
    #[serde(default)]
    body: String,
}

pub(crate) async fn handler_comment_post(State(state): State_, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(form) = serde_json::from_slice::<CommentBody>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let owner = match authorize(&state, &headers, &form.repo, Tier::Reporter).await {
        Ok(owner) => owner,
        Err(resp) => return *resp,
    };
    match state
        .core
        .comment_merge_request(&form.repo, form.number, &owner, form.body.trim())
        .await
    {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct MergeBody {
    repo: String,
    number: u64,
    #[serde(default)]
    strategy: gitquarry_wire::MergeStrategy,
}

pub(crate) async fn handler_merge_post(State(state): State_, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(form) = serde_json::from_slice::<MergeBody>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let owner = match authorize(&state, &headers, &form.repo, Tier::Developer).await {
        Ok(owner) => owner,
        Err(resp) => return *resp,
    };
    match state
        .core
        .merge_merge_request(&form.repo, form.number, &owner, form.strategy)
        .await
    {
        Ok(outcome) => json_response(StatusCode::OK, &outcome),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}
