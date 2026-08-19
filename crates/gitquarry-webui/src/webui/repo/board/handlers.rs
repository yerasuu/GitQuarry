//! The `/_board*` mutation handlers [`super`]'s view POSTs to. Top-level
//! routes, not nested under the repo's own `/{*repo}` path — same shape as
//! `/_issues`/`/_milestones` (`milestones::handlers`'s own doc comment
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
use gitquarry_wire::tracker::{CardRefDto, ReorderDirection};

fn tracker_unavailable(locale: Locale) -> AppError {
    AppError::service_unavailable(anyhow::anyhow!(
        "{}",
        gitquarry_i18n::shared::tracker_not_configured(locale)
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
            gitquarry_i18n::shared::repo_not_found(locale, &name)
        )));
    };
    Ok((resolved, name))
}

/// Same bar `milestones::handlers::require_developer_permission` gates
/// with — a board is repo management, not owned by whoever created a card
/// (see `super::can_manage`'s own doc comment).
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
            gitquarry_i18n::shared::write_permission_required(locale)
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
pub(crate) struct AddColumnForm {
    repo: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_add_column_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<AddColumnForm>,
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

    let ok = tracker_client::add_board_column(tracker, &resolved.uid, &form.name)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "board.add_column",
        &name,
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "board", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct ColumnActionForm {
    repo: String,
    column: usize,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_remove_column_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<ColumnActionForm>,
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

    let ok = tracker_client::remove_board_column(tracker, &resolved.uid, form.column)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "board.remove_column",
        &name,
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "board", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct AddCardForm {
    repo: String,
    column: usize,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_add_card_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<AddCardForm>,
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

    let card = match form.kind.as_str() {
        "issue" => {
            let number = form.value.trim().parse::<u64>().map_err(|_| {
                AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::board_card_target_not_found(locale)
                ))
            })?;
            CardRefDto::Issue(number)
        }
        "pull_request" => {
            let number = form.value.trim().parse::<u64>().map_err(|_| {
                AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::board_card_target_not_found(locale)
                ))
            })?;
            CardRefDto::PullRequest(number)
        }
        _ => CardRefDto::Note(form.value.clone()),
    };

    let ok = tracker_client::add_board_card(tracker, &resolved.uid, form.column, card)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "board.add_card",
        &name,
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "board", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct CardActionForm {
    repo: String,
    card_id: u64,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_remove_card_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<CardActionForm>,
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

    let ok = tracker_client::remove_board_card(tracker, &resolved.uid, form.card_id)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "board.remove_card",
        &name,
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "board", "").to_string()).into_response())
}

pub(crate) async fn handler_move_card_up_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<CardActionForm>,
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

    let ok = tracker_client::reorder_board_card(tracker, &resolved.uid, form.card_id, ReorderDirection::Up)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;
    Ok(Redirect::to(&repo_link(&name, "board", "").to_string()).into_response())
}

pub(crate) async fn handler_move_card_down_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<CardActionForm>,
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

    let ok = tracker_client::reorder_board_card(tracker, &resolved.uid, form.card_id, ReorderDirection::Down)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;
    Ok(Redirect::to(&repo_link(&name, "board", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct MoveToColumnForm {
    repo: String,
    card_id: u64,
    to_column: usize,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_move_card_to_column_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<MoveToColumnForm>,
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

    let ok = tracker_client::move_board_card(tracker, &resolved.uid, form.card_id, form.to_column)
        .await
        .map_err(AppError::bad_request)?;
    check_ok(ok, locale)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "board.move_card",
        &name,
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "board", "").to_string()).into_response())
}
