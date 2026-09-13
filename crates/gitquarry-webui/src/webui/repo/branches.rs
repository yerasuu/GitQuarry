//! `POST /_branches/delete`: delete a branch. Top-level route, not nested
//! under the repo's own `/{*repo}` path — same reason as [`super::tags`]'s
//! own routes (see that module's doc comment): the repo name arrives as a
//! hidden form field, not a path segment. No create route — unlike a tag, a
//! branch only ever comes into existence via a real push.

use crate::webui::{AppError, State_, effective_locale, repo_link, require_csrf, require_login, url_encode};
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::domain;
use gitquarry_core::git;
use gitquarry_core::storage::Cas;
use gitquarry_core::validate;

/// Same gate and same "the actual mutation stays re-checked here" reasoning
/// as [`super::tags::require_write_target`], duplicated rather than shared
/// for the same reason that one gives.
async fn require_write_target(
    ctx: &AppContext,
    me: &User,
    repo: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(ResolvedRepo, String), AppError> {
    let name = validate::sanitize_path(repo).map_err(AppError::bad_request)?;
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &name)
        )));
    };
    let perm =
        gitquarry_core::auth::access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, me, &name)
            .await?;
    if perm < Some(Permission::Developer) {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::write_permission_required(locale)
        )));
    }
    Ok((resolved, name))
}

#[derive(serde::Deserialize)]
pub(crate) struct BranchDeleteForm {
    branch: String,
    #[serde(default)]
    csrf: String,
}

/// `GET` on this POST-only mutation target: what a browser asks for when it
/// reloads or backs into the URL a rejected submit left in the address bar.
/// The repo is in the path, so it can land on the repo's refs page, where the branch controls live
/// rather than on the router's generic "back to the index" fallback.
pub(crate) async fn handler_mutation_get(Path(repo): Path<String>) -> Response {
    axum::response::Redirect::to(&format!("/{}?view=refs", url_encode(&repo))).into_response()
}

pub(crate) async fn handler_delete_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<BranchDeleteForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let branch = form.branch.trim();
    let cas = domain::repo::delete_branch(&resolved, branch, locale)
        .await
        .map_err(AppError::bad_request)?;
    if cas == Cas::Conflict {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "branch.delete",
        &format!("{name}@{branch}"),
        None,
    )
    .await;
    // Async, same "re-render the whole table, not just this row" reasoning
    // as `tags::handler_delete_post`.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        let mirror = ctx.open_mirror_read(&resolved).await?;
        let branches = git::browse::list_refs_detailed(&mirror.path, "refs/heads/").await?;
        return Ok(axum::response::Html(
            super::refs::branch_table(&name, &branches, true, &headers, locale).into_string(),
        )
        .into_response());
    }
    Ok(Redirect::to(&repo_link(&name, "refs", "").to_string()).into_response())
}
