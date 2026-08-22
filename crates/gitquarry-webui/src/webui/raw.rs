//! Raw-export views (`/_raw/{kind}/{repo}`): blob bytes, a mailable patch,
//! a raw diff, snapshot archives, the atom feed, and release assets. Split
//! off `repo::handler_repo`'s `?view=...` dispatch onto their own route
//! tree so a raw response can never share a code path with an HTML page —
//! every handler here returns a bare [`Response`], never `AppError`, so
//! there's no `IntoResponse for AppError` to fall back to an HTML error
//! page on a bad ref, a missing blob, or anything else that goes wrong.

use super::compound_path::resolve_compound_path;
use super::repo::{can_view_repo, require_filename, require_sha, require_tag};
use super::{AppError, State_, effective_locale, export};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use gitquarry_core::app::{AppContext, Mirror, ResolvedRepo};
use gitquarry_core::{git, validate};
use std::collections::HashMap;

async fn resolve_visible_repo(
    ctx: &AppContext,
    headers: &HeaderMap,
    raw_repo: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(String, ResolvedRepo), AppError> {
    let name = validate::sanitize_path(raw_repo).map_err(AppError::bad_request)?;
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, &name)
        )));
    };
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if !can_view_repo(ctx, headers, &name, config.visibility).await? {
        // Same response as "doesn't exist": doesn't confirm a private repo
        // is merely hidden vs. genuinely absent.
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, &name)
        )));
    }
    Ok((name, resolved))
}

/// Opens the mirror and resolves its default branch — every raw kind that
/// reads from the git tree (as opposed to `release_asset`, which reads S3
/// directly) needs both.
async fn resolve_mirror_and_default_rev(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    name: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(Mirror, String), AppError> {
    let mirror = ctx.open_mirror(resolved).await?;
    let Some(head) = git::browse::resolve_head(&mirror.path).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_has_no_commits(locale, name)
        )));
    };
    let default_rev = head.strip_prefix("refs/heads/").unwrap_or(&head).to_string();
    Ok((mirror, default_rev))
}

pub(crate) async fn handler_raw_plain(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let locale = effective_locale(&ctx, &headers).await;
    let result: Result<Response, AppError> = async {
        // GitHub-style `.../refs/heads/{branch...}/{path...}` embedded in
        // the wildcard capture, tried first — see `compound_path`'s module
        // doc for why the split can't just assume a fixed segment count.
        if let Some(compound) = resolve_compound_path(&ctx, &headers, &raw_repo, &["refs", "heads"]).await? {
            return export::plain(&compound.mirror.path, &compound.branch, &compound.path, locale).await;
        }
        let (name, resolved) = resolve_visible_repo(&ctx, &headers, &raw_repo, locale).await?;
        let (mirror, default_rev) = resolve_mirror_and_default_rev(&ctx, &resolved, &name, locale).await?;
        let rev = params.get("ref").map_or(default_rev.as_str(), String::as_str);
        let path = params.get("path").map_or("", String::as_str);
        validate::validate_rev(rev).map_err(AppError::bad_request)?;
        validate::validate_tree_path(path).map_err(AppError::bad_request)?;
        export::plain(&mirror.path, rev, path, locale).await
    }
    .await;
    result.unwrap_or_else(AppError::into_plain_response)
}

pub(crate) async fn handler_raw_patch(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let locale = effective_locale(&ctx, &headers).await;
    let result: Result<Response, AppError> = async {
        let (_name, resolved) = resolve_visible_repo(&ctx, &headers, &raw_repo, locale).await?;
        let mirror = ctx.open_mirror(&resolved).await?;
        let sha = require_sha(&params, locale)?;
        export::patch(&mirror.path, sha).await
    }
    .await;
    result.unwrap_or_else(AppError::into_plain_response)
}

pub(crate) async fn handler_raw_rawdiff(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let locale = effective_locale(&ctx, &headers).await;
    let result: Result<Response, AppError> = async {
        let (_name, resolved) = resolve_visible_repo(&ctx, &headers, &raw_repo, locale).await?;
        let mirror = ctx.open_mirror(&resolved).await?;
        let sha = require_sha(&params, locale)?;
        export::rawdiff(&mirror.path, sha).await
    }
    .await;
    result.unwrap_or_else(AppError::into_plain_response)
}

pub(crate) async fn handler_raw_snapshot(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let locale = effective_locale(&ctx, &headers).await;
    let result: Result<Response, AppError> = async {
        let (name, resolved) = resolve_visible_repo(&ctx, &headers, &raw_repo, locale).await?;
        let (mirror, default_rev) = resolve_mirror_and_default_rev(&ctx, &resolved, &name, locale).await?;
        let rev = params.get("ref").map_or(default_rev.as_str(), String::as_str);
        let fmt = params.get("fmt").map_or("tar.gz", String::as_str);
        validate::validate_rev(rev).map_err(AppError::bad_request)?;
        // Dropped before archiving, not held through it: `git archive` over
        // a large tree can take a while, and every other view of this repo
        // waits on this same per-repo lock — same reasoning as
        // `webui::repo::releases::generate_source_archives`'s doc comment.
        let git_dir = mirror.path.clone();
        drop(mirror);
        export::snapshot(&git_dir, &name, rev, fmt, locale).await
    }
    .await;
    result.unwrap_or_else(AppError::into_plain_response)
}

pub(crate) async fn handler_raw_atom(State(ctx): State_, Path(raw_repo): Path<String>, headers: HeaderMap) -> Response {
    let locale = effective_locale(&ctx, &headers).await;
    let result: Result<Response, AppError> = async {
        let (name, resolved) = resolve_visible_repo(&ctx, &headers, &raw_repo, locale).await?;
        let (mirror, default_rev) = resolve_mirror_and_default_rev(&ctx, &resolved, &name, locale).await?;
        export::atom(&mirror.path, &name, &default_rev).await
    }
    .await;
    result.unwrap_or_else(AppError::into_plain_response)
}

pub(crate) async fn handler_raw_release_asset(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let locale = effective_locale(&ctx, &headers).await;
    let result: Result<Response, AppError> = async {
        let (_name, resolved) = resolve_visible_repo(&ctx, &headers, &raw_repo, locale).await?;
        let tag = require_tag(&params, locale)?;
        let filename = require_filename(&params, locale)?;
        export::release_asset(&resolved.remote, tag, filename, locale).await
    }
    .await;
    result.unwrap_or_else(AppError::into_plain_response)
}
