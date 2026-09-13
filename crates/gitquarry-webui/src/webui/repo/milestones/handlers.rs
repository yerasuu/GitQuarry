//! The `/_milestones*` mutation handlers [`super`]'s view POSTs to.
//! Top-level routes, not nested under the repo's own `/{*repo}` path —
//! same shape as `/_issues`/`/_pulls` (`pulls::handlers`'s own doc comment
//! explains why).

use crate::webui::{
    AppError, State_, effective_locale, repo_link, require_csrf, require_login, tracker_client, url_encode,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::validate;
use gitquarry_i18n::Locale;

fn tracker_unavailable(locale: Locale) -> AppError {
    AppError::service_unavailable(anyhow::anyhow!(
        "{}",
        gitquarry_i18n::shared::tracker::tracker_not_configured(locale)
    ))
}

async fn resolve_target(
    ctx: &AppContext,
    repo: &str,
    locale: Locale,
) -> Result<(gitquarry_core::app::ResolvedRepo, String), AppError> {
    let name = validate::sanitize_path(repo).map_err(AppError::bad_request)?;
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &name)
        )));
    };
    Ok((resolved, name))
}

/// `Err` if `me` isn't at least `Developer` on `repo_name` — same bar
/// [`crate::webui::repo::pulls::handlers`] gates merging with, reused here
/// since a milestone is repo management, not owned by whoever created it
/// (see [`super::can_manage`]'s own doc comment).
async fn require_developer_permission(
    ctx: &AppContext,
    me: &User,
    repo_name: &str,
    locale: Locale,
) -> Result<(), AppError> {
    let perm =
        gitquarry_core::auth::access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, me, repo_name)
            .await?;
    if perm < Some(Permission::Developer) {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::write_permission_required(locale)
        )));
    }
    Ok(())
}

fn check_ok(ok: bool, locale: Locale) -> Result<(), AppError> {
    if !ok {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateMilestoneForm {
    repo: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    due_date: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_create_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<CreateMilestoneForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &form.repo, locale).await?;
    require_developer_permission(&ctx, &me, &name, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let milestone =
        tracker_client::create_milestone(tracker, &resolved.uid, &form.title, &form.description, &form.due_date)
            .await
            .map_err(AppError::bad_request)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "milestone.create",
        &format!("{name}#{}", milestone.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "milestones", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct EditMilestoneForm {
    repo: String,
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    due_date: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_edit_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<EditMilestoneForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &form.repo, locale).await?;
    require_developer_permission(&ctx, &me, &name, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let ok = tracker_client::edit_milestone(
        tracker,
        &resolved.uid,
        form.number,
        &form.title,
        &form.description,
        &form.due_date,
    )
    .await
    .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "milestone.edit",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "milestones", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct MilestoneActionForm {
    repo: String,
    number: u64,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_close_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<MilestoneActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &form.repo, locale).await?;
    require_developer_permission(&ctx, &me, &name, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let ok = tracker_client::close_milestone(tracker, &resolved.uid, form.number)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "milestone.close",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "milestones", "").to_string()).into_response())
}

pub(crate) async fn handler_reopen_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<MilestoneActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &form.repo, locale).await?;
    require_developer_permission(&ctx, &me, &name, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let ok = tracker_client::reopen_milestone(tracker, &resolved.uid, form.number)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "milestone.reopen",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "milestones", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct BulkMilestoneActionForm {
    repo: String,
    /// Checkbox values (`<input type="checkbox" name="numbers" value=...>`),
    /// repeated once per selected row — same shape
    /// `issues::handlers::BulkIssueActionForm` already uses.
    #[serde(default)]
    numbers: Vec<u64>,
    /// `"close"` or `"reopen"` — anything else is rejected up front, before
    /// touching any milestone.
    action: String,
    #[serde(default)]
    csrf: String,
}

/// Best-effort per milestone, same tolerance every other bulk handler in
/// this codebase already accepts: one already-in-that-state milestone in
/// the selection doesn't abort the rest of the batch.
pub(crate) async fn handler_bulk_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<BulkMilestoneActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &form.repo, locale).await?;
    require_developer_permission(&ctx, &me, &name, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    if !matches!(form.action.as_str(), "close" | "reopen") {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::unknown_action(locale, &form.action)
        )));
    }
    let mut applied = 0usize;
    for number in &form.numbers {
        let result = if form.action == "close" {
            tracker_client::close_milestone(tracker, &resolved.uid, *number).await
        } else {
            tracker_client::reopen_milestone(tracker, &resolved.uid, *number).await
        };
        if result.is_ok_and(|ok| ok) {
            applied += 1;
        }
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        &format!("milestone.bulk_{}", form.action),
        &name,
        Some(format!("{applied}/{} milestone(s)", form.numbers.len())),
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "milestones", "").to_string()).into_response())
}
