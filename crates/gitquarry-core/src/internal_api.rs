//! Loopback-only `/_internal/*` API — what a `gitquarry-federation` process
//! calls into core for (`docs/federation-binary-split-concept.md` section
//! 3B). Registered only when `[federation]` is present in `gitquarry.toml`
//! (`config::FederationSection`); bound to `internal_bind` (an operator's
//! responsibility to keep that loopback-only) and, defense in depth, every
//! request must also carry the shared secret in the `X-Internal-Token`
//! header matching `internal_token` — the same secret
//! `gitquarry-federation.toml` is manually configured with.
//!
//! `gitquarry-federation` never receives a [`crate::auth::entities::Permission`]
//! or [`crate::auth::entities::User`] over this API: every access decision
//! (repo permission tier, namespace ownership for fork-from-remote) is made
//! here, in core, the source of truth, never cached or re-derived on the
//! federation side. `MergeRequestMeta`/`MrDetail`/`RepoSummary` are
//! `gitquarry_wire` types shared verbatim across the boundary; this module
//! never depends on `gitquarry-federation` itself (wrong direction — core
//! must not know federation exists beyond `AppContext::federation`'s own
//! outbound link), so a couple of small response shapes below intentionally
//! mirror federation's own `core_client.rs` types rather than importing
//! them.

use crate::app::{AppContext, ResolvedRepo};
use crate::auth::access;
use crate::auth::entities::Permission;
use crate::domain::merge_request;
use crate::domain::remote_fork::{SeedFromRemote, seed_from_remote};
use crate::storage;
use crate::storage::entities::{OwnerRef, Visibility};
use crate::storage::uid::Uid;
use anyhow::{Context, Result};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use gitquarry_i18n::Locale;
use gitquarry_wire::{MrDetail, RepoSummary};
use serde::{Deserialize, Serialize};
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
        .route("/_internal/authorize", post(handler_authorize))
        .route(
            "/_internal/repos/authorize-and-summarize",
            post(handler_authorize_and_summarize),
        )
        .route("/_internal/repos/public", get(handler_public_repos))
        .route("/_internal/repos/public/{*name}", get(handler_public_repo))
        .route("/_internal/repos/uid-for-name", get(handler_uid_for_name))
        .route("/_internal/merge-requests/list", get(handler_mr_list))
        .route(
            "/_internal/merge-requests",
            get(handler_mr_detail).post(handler_mr_create),
        )
        .route("/_internal/merge-requests/comment", post(handler_mr_comment))
        .route("/_internal/merge-requests/merge", post(handler_mr_merge))
        .route("/_internal/repos/fork-from-remote", post(handler_fork_from_remote))
        .with_state(ctx)
}

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(axum::http::header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

fn json_error(status: StatusCode, message: &str) -> Response {
    gitquarry_wire::ipc::json_error(status, message)
}

fn repo_summary(config: &storage::entities::RepoConfig) -> RepoSummary {
    RepoSummary {
        name: config.full_name.clone(),
        description: config.description.clone(),
        topics: config.topics.clone(),
        star_count: config.star_count,
        fork_count: config.fork_count,
    }
}

#[derive(Deserialize)]
struct AuthorizeRequest {
    username: String,
    repo: String,
    require: Permission,
}

#[derive(Serialize)]
struct AuthorizeResponse {
    ok: bool,
}

/// `POST /_internal/authorize` — the one place a real access decision ever
/// gets made for federation's bearer-token layer; see `entities::AccessToken`'s
/// own doc comment (in `gitquarry-federation`) on why that crate never
/// computes a [`Permission`] itself.
pub async fn handler_authorize(State(ctx): State<Arc<AppContext>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<AuthorizeRequest>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let ok = match access::effective_permission_for(ctx.backend().as_ref(), &ctx.cipher, &req.username, &req.repo).await
    {
        Ok(perm) => perm >= Some(req.require),
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    json_response(StatusCode::OK, &AuthorizeResponse { ok })
}

#[derive(Deserialize)]
struct AuthorizeAndSummarizeRequest {
    username: String,
    uid: Uid,
    require: Permission,
}

#[derive(Serialize)]
struct AuthorizeAndSummarizeResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<RepoSummary>,
}

/// `POST /_internal/repos/authorize-and-summarize` — the token-scoped
/// equivalent of [`handler_authorize`] that also hands back the repo's
/// public metadata, for a `private`/`limited` repo a fine-grained
/// `AccessToken` lists by uid. `repo` omitted entirely (not `null`) unless
/// `ok` is true — never returns a summary without also having verified the
/// caller is entitled to it.
pub async fn handler_authorize_and_summarize(State(ctx): State<Arc<AppContext>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<AuthorizeAndSummarizeRequest>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let (config, etag) =
        match storage::settings::get_repo_config(ctx.backend().as_ref(), req.uid.as_str(), &ctx.cipher).await {
            Ok(result) => result,
            Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
        };
    if etag.is_none() {
        return json_response(StatusCode::OK, &AuthorizeAndSummarizeResponse { ok: false, repo: None });
    }
    let ok =
        match access::effective_permission_for(ctx.backend().as_ref(), &ctx.cipher, &req.username, &config.full_name)
            .await
        {
            Ok(perm) => perm >= Some(req.require),
            Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
        };
    json_response(
        StatusCode::OK,
        &AuthorizeAndSummarizeResponse {
            ok,
            repo: ok.then(|| repo_summary(&config)),
        },
    )
}

/// `GET /_internal/repos/public` — every repo core currently exposes
/// publicly. Same visibility-index-then-recheck pattern
/// `webui::repo::list_visible_repos` uses: the index is a hint, never
/// trusted without re-reading each candidate's own `RepoConfig`.
pub async fn handler_public_repos(State(ctx): State<Arc<AppContext>>) -> Response {
    let backend = ctx.backend();
    let uids = match storage::settings::list_visible_repo_uids(backend.as_ref()).await {
        Ok(uids) => uids,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let mut repos = Vec::new();
    for uid in uids {
        let Ok((config, etag)) = storage::settings::get_repo_config(backend.as_ref(), uid.as_str(), &ctx.cipher).await
        else {
            continue;
        };
        if etag.is_none() || config.deleted_at.is_some() || config.visibility != Visibility::Public {
            continue;
        }
        repos.push(repo_summary(&config));
    }
    json_response(StatusCode::OK, &repos)
}

/// `GET /_internal/repos/public/{name}` — `None` (404) if `name` doesn't
/// exist or isn't [`Visibility::Public`]. `name` is a path segment, not a
/// query param, since a repo's own full name already contains a `/`
/// (`owner/repo`) — matches `core_client::CoreClient::public_repo`'s own
/// unencoded `/repos/public/{name}` path.
pub async fn handler_public_repo(State(ctx): State<Arc<AppContext>>, Path(name): Path<String>) -> Response {
    let Ok(Some(uid)) = storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, &name).await else {
        return json_error(StatusCode::NOT_FOUND, "no such repo");
    };
    let Ok((config, etag)) =
        storage::settings::get_repo_config(ctx.backend().as_ref(), uid.as_str(), &ctx.cipher).await
    else {
        return json_error(StatusCode::NOT_FOUND, "no such repo");
    };
    if etag.is_none() || config.visibility != Visibility::Public {
        return json_error(StatusCode::NOT_FOUND, "no such repo");
    }
    json_response(StatusCode::OK, &repo_summary(&config))
}

#[derive(Serialize)]
struct UidForNameResponse {
    uid: String,
}

/// `GET /_internal/repos/uid-for-name?name=` — see
/// `entities::AccessToken::repos`'s own doc comment (in `gitquarry-federation`)
/// on why this is needed at all: a token scopes itself by uid, but every
/// external MR-proxy call carries a repo *name*.
pub async fn handler_uid_for_name(
    State(ctx): State<Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(name) = params.get("name") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?name=");
    };
    match storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, name).await {
        Ok(Some(uid)) => json_response(
            StatusCode::OK,
            &UidForNameResponse {
                uid: uid.as_str().to_string(),
            },
        ),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "no such repo"),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn resolved_repo_or_404(ctx: &AppContext, repo: &str) -> Result<ResolvedRepo, Response> {
    match ctx.resolve_repo(repo).await {
        Ok(Some(resolved)) => Ok(resolved),
        Ok(None) => Err(json_error(StatusCode::NOT_FOUND, "no such repo")),
        Err(e) => Err(json_error(StatusCode::BAD_REQUEST, &e.to_string())),
    }
}

/// `GET /_internal/merge-requests/list?repo=` — every MR on `repo`.
pub async fn handler_mr_list(
    State(ctx): State<Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(repo) = params.get("repo") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?repo=");
    };
    let target = match resolved_repo_or_404(&ctx, repo).await {
        Ok(target) => target,
        Err(resp) => return resp,
    };
    match merge_request::list(&ctx, &target, None).await {
        Ok(mrs) => json_response(StatusCode::OK, &mrs),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// `GET /_internal/merge-requests?repo=&number=` — one MR plus its comments
/// and reviews.
pub async fn handler_mr_detail(
    State(ctx): State<Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(repo) = params.get("repo") else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?repo=");
    };
    let Some(number) = params.get("number").and_then(|s| s.parse::<u64>().ok()) else {
        return json_error(StatusCode::BAD_REQUEST, "missing or invalid ?number=");
    };
    let target = match resolved_repo_or_404(&ctx, repo).await {
        Ok(target) => target,
        Err(resp) => return resp,
    };
    let mr = match merge_request::get(&ctx, &target, number).await {
        Ok(Some(mr)) => mr,
        Ok(None) => return json_error(StatusCode::NOT_FOUND, "no such merge request"),
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let comments = match merge_request::list_comments(&ctx, &target, number).await {
        Ok(comments) => comments.into_iter().map(|(_key, comment)| comment).collect(),
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let reviews = match merge_request::list_reviews(&ctx, &target, number).await {
        Ok(reviews) => reviews,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    json_response(StatusCode::OK, &MrDetail { mr, comments, reviews })
}

#[derive(Deserialize)]
struct CreateMergeRequestBody {
    repo: String,
    source_repo: String,
    source_branch: String,
    target_branch: String,
    title: String,
    description: String,
    author: String,
}

/// `POST /_internal/merge-requests` — caller (`gitquarry-federation`'s
/// `web::api_pulls::handler_create_post`) has already authorized `author` on
/// `repo` via `POST /_internal/authorize`, same division of labor this
/// module's own doc comment describes.
pub async fn handler_mr_create(State(ctx): State<Arc<AppContext>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<CreateMergeRequestBody>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let target = match resolved_repo_or_404(&ctx, &req.repo).await {
        Ok(target) => target,
        Err(resp) => return resp,
    };
    let source_uid =
        match storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, &req.source_repo).await {
            Ok(Some(uid)) => uid,
            Ok(None) => return json_error(StatusCode::NOT_FOUND, "no such source repo"),
            Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
        };
    let new = merge_request::NewMergeRequest {
        source_repo: &source_uid,
        source_branch: &req.source_branch,
        target_branch: &req.target_branch,
        title: &req.title,
        description: &req.description,
        author: &req.author,
    };
    match merge_request::create(&ctx, &target, new, Locale::DEFAULT).await {
        Ok(mr) => json_response(StatusCode::OK, &mr),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct CommentMergeRequestBody {
    repo: String,
    number: u64,
    author: String,
    body: String,
}

pub async fn handler_mr_comment(State(ctx): State<Arc<AppContext>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<CommentMergeRequestBody>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let target = match resolved_repo_or_404(&ctx, &req.repo).await {
        Ok(target) => target,
        Err(resp) => return resp,
    };
    match merge_request::add_comment(&ctx, &target, req.number, &req.author, &req.body, Locale::DEFAULT).await {
        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct MergeMergeRequestBody {
    repo: String,
    number: u64,
    merged_by: String,
    #[serde(default)]
    strategy: gitquarry_wire::MergeStrategy,
}

/// Mirrors federation's own `core_client::MergeResult` (an `untagged` enum)
/// without depending on `gitquarry-federation` — see this module's own doc
/// comment on why the two crates each keep a small local copy of shapes
/// that cross the boundary.
#[derive(Serialize)]
#[serde(untagged)]
enum MergeResultResponse {
    Merged {
        merged: bool,
        sha: String,
    },
    Conflict {
        merged: bool,
        conflicting_paths: Vec<String>,
    },
}

pub async fn handler_mr_merge(State(ctx): State<Arc<AppContext>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<MergeMergeRequestBody>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    let target = match resolved_repo_or_404(&ctx, &req.repo).await {
        Ok(target) => target,
        Err(resp) => return resp,
    };
    match merge_request::merge(&ctx, &target, req.number, &req.merged_by, req.strategy, Locale::DEFAULT).await {
        Ok(merge_request::MergeOutcome::Merged(sha)) => {
            json_response(StatusCode::OK, &MergeResultResponse::Merged { merged: true, sha })
        }
        Ok(merge_request::MergeOutcome::Conflict(conflicting_paths)) => json_response(
            StatusCode::OK,
            &MergeResultResponse::Conflict {
                merged: false,
                conflicting_paths,
            },
        ),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct ForkFromRemoteRequest {
    source_git_base_url: String,
    source_repo_name: String,
    dest_full_name: String,
    /// `"user"` or `"group"` — matches [`OwnerRef`]'s own
    /// `#[serde(rename_all = "lowercase")]` tag, since `gitquarry-federation`
    /// has no direct dependency on this crate's storage types and sends the
    /// tag as a plain string instead.
    dest_owner_kind: String,
    dest_owner_uid: Uid,
    description: Option<String>,
    peer_label: String,
    acting_username: String,
}

#[derive(Serialize)]
struct ForkOutcomeResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

fn fork_rejected(reason: impl Into<String>) -> Response {
    json_response(
        StatusCode::OK,
        &ForkOutcomeResponse {
            ok: false,
            repo_uid: None,
            reason: Some(reason.into()),
        },
    )
}

/// Whether `acting_username` controls the namespace `dest_owner_kind`
/// `dest_owner_uid` names — their own user uid, or a group they hold at
/// least [`Permission::Owner`] on (anywhere in its ancestor chain, same bar
/// [`access::can_manage_group`] applies everywhere else). Checked here,
/// never trusted from the federation side: `acting_username` reaches this
/// handler having already authenticated to federation's own public API, but
/// federation itself has no way to know what namespaces that user actually
/// controls in this bucket.
async fn acting_user_controls_namespace(
    ctx: &AppContext,
    acting_username: &str,
    dest_owner_kind: &str,
    dest_owner_uid: &Uid,
) -> Result<bool> {
    let backend = ctx.backend();
    let Some(acting_user) = access::get_user(backend.as_ref(), &ctx.cipher, acting_username)
        .await?
        .0
    else {
        return Ok(false);
    };
    match dest_owner_kind {
        "user" => Ok(acting_user.is_admin || &acting_user.uid == dest_owner_uid),
        "group" => {
            let Some(group) = access::resolve_group_uid(backend.as_ref(), &ctx.cipher, dest_owner_uid).await? else {
                return Ok(false);
            };
            let chain = access::ancestor_group_chain(backend.as_ref(), &ctx.cipher, &group.name).await?;
            Ok(access::can_manage_group(Some(&acting_user), &chain))
        }
        _ => Ok(false),
    }
}

/// `POST /_internal/repos/fork-from-remote` — see `crate::domain::remote_fork`
/// for the actual git/storage mechanic; this handler's whole job is the
/// authorization check [`acting_user_controls_namespace`] performs before
/// ever calling it, plus turning `dest_owner_kind`/`dest_owner_uid` into the
/// [`OwnerRef`] that expects.
pub async fn handler_fork_from_remote(State(ctx): State<Arc<AppContext>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<ForkFromRemoteRequest>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid JSON body");
    };
    match acting_user_controls_namespace(&ctx, &req.acting_username, &req.dest_owner_kind, &req.dest_owner_uid).await {
        Ok(true) => {}
        Ok(false) => {
            return fork_rejected(format!("{} doesn't control that namespace", req.acting_username));
        }
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
    let dest_owner = match req.dest_owner_kind.as_str() {
        "user" => OwnerRef::User(req.dest_owner_uid.clone()),
        "group" => OwnerRef::Group(req.dest_owner_uid.clone()),
        _ => return json_error(StatusCode::BAD_REQUEST, "invalid dest_owner_kind"),
    };
    let seed = SeedFromRemote {
        source_git_base_url: &req.source_git_base_url,
        source_repo_name: &req.source_repo_name,
        dest_full_name: &req.dest_full_name,
        dest_owner,
        description: req.description,
        peer_label: &req.peer_label,
    };
    match seed_from_remote(&ctx, seed, Locale::DEFAULT).await {
        Ok(Some(resolved)) => json_response(
            StatusCode::OK,
            &ForkOutcomeResponse {
                ok: true,
                repo_uid: Some(resolved.uid.as_str().to_string()),
                reason: None,
            },
        ),
        Ok(None) => fork_rejected(format!("{} already exists", req.dest_full_name)),
        Err(e) => fork_rejected(e.to_string()),
    }
}
