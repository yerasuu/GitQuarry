//! The `/_issues*` mutation handlers the views in [`super`] POST to.
//! Top-level routes, not nested under the repo's own `/{*repo}` path — see
//! [`super`]'s own doc comment on why.

use crate::auth::web::login;
use crate::webui::repo::can_view_repo;
use crate::webui::{
    ASYNC_HEADER, AppError, State_, csrf_field, effective_density, effective_locale, effective_theme, layout,
    repo_link, require_csrf, require_login, tracker_client, url_encode,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::validate;
use gitquarry_i18n::Locale;

fn tracker_unavailable(locale: Locale) -> AppError {
    AppError::service_unavailable(anyhow::anyhow!(
        "{}",
        gitquarry_i18n::shared::tracker_not_configured(locale)
    ))
}

/// Resolves `repo` and checks the caller can actually see it — same
/// reasoning as `pulls::handlers::resolve_target`: filing or commenting on
/// an issue needs only view access, but these are top-level routes that
/// `handler_repo`'s own [`can_view_repo`] gate never runs in front of, so
/// the check has to happen here or not at all. Close/reopen/edit check the
/// issue's own author-or-`Developer` bar on top of this, via
/// [`require_manage_permission`] below. A rejection is 404, not 403, so a
/// refused write doesn't confirm that a private repo exists.
async fn resolve_target(
    ctx: &AppContext,
    headers: &HeaderMap,
    repo: &str,
    locale: Locale,
) -> Result<(ResolvedRepo, String), AppError> {
    let name = validate::sanitize_path(repo).map_err(AppError::bad_request)?;
    let not_found = || {
        AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo_not_found(locale, &name)
        ))
    };
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(not_found());
    };
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if !can_view_repo(ctx, headers, &name, config.visibility).await? {
        return Err(not_found());
    }
    Ok((resolved, name))
}

/// `Err` if `number` doesn't exist, or `me` is neither its author nor
/// `Developer`+ on `repo_name` — same shape as
/// `pulls::handlers::require_manage_permission`.
async fn require_manage_permission(
    ctx: &AppContext,
    me: &User,
    target: &ResolvedRepo,
    repo_name: &str,
    number: u64,
    locale: Locale,
) -> Result<(), AppError> {
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };
    let Some(issue) = tracker_client::get_issue(tracker, &target.uid, number).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::issue_not_found(locale, number)
        )));
    };
    if issue.author == me.username {
        return Ok(());
    }
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

#[derive(serde::Deserialize)]
pub(crate) struct NewIssueForm {
    repo: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_create_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<NewIssueForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let issue = match tracker_client::create_issue(tracker, &resolved.uid, &form.title, &form.body, &me.username).await
    {
        Ok(issue) => issue,
        Err(e) => {
            let topnav = login::topnav(&ctx, &headers).await;
            let body = super::render_new_issue_form(
                &name,
                None,
                &form.title,
                &form.body,
                Some(&e.to_string()),
                &headers,
                locale,
            )
            .into_string();
            return Ok(Html(layout(
                "new issue",
                "",
                &body,
                &topnav,
                effective_theme(&ctx, &headers).await,
                effective_density(&ctx, &headers).await,
                locale,
            ))
            .into_response());
        }
    };

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "issue.create",
        &format!("{name}#{}", issue.number),
        None,
    )
    .await;

    Ok(Redirect::to(&repo_link(&name, "issue", "").number(issue.number).to_string()).into_response())
}

/// Async (`data-async`/`data-async-target="#issue-actions"`, see
/// `ASYNC_HEADER`'s own doc comment) response for close/reopen: the
/// `#issue-actions` fragment reflecting whatever the issue's state is *now*
/// (re-fetched, not assumed from the request that just mutated it) — same
/// shape as `pulls::handlers::async_pr_actions_response`.
async fn async_issue_actions_response(
    ctx: &AppContext,
    headers: &HeaderMap,
    resolved: &ResolvedRepo,
    repo_name: &str,
    number: u64,
    locale: Locale,
) -> Result<Response, AppError> {
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };
    let Some(issue) = tracker_client::get_issue(tracker, &resolved.uid, number).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::issue_not_found(locale, number)
        )));
    };
    let can_manage = super::can_manage(ctx, headers, repo_name, &issue.author).await?;
    Ok(
        Html(super::render_issue_actions(&issue, can_manage, repo_name, &csrf_field(headers), locale).into_string())
            .into_response(),
    )
}

#[derive(serde::Deserialize)]
pub(crate) struct IssueActionForm {
    repo: String,
    number: u64,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_close_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<IssueActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let ok = tracker_client::close_issue(tracker, &resolved.uid, form.number)
        .await
        .map_err(AppError::bad_request)?;
    if !ok {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "issue.close",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    if headers.contains_key(ASYNC_HEADER) {
        return async_issue_actions_response(&ctx, &headers, &resolved, &name, form.number, locale).await;
    }
    Ok(Redirect::to(&repo_link(&name, "issue", "").number(form.number).to_string()).into_response())
}

pub(crate) async fn handler_reopen_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<IssueActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let ok = tracker_client::reopen_issue(tracker, &resolved.uid, form.number)
        .await
        .map_err(AppError::bad_request)?;
    if !ok {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "issue.reopen",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    if headers.contains_key(ASYNC_HEADER) {
        return async_issue_actions_response(&ctx, &headers, &resolved, &name, form.number, locale).await;
    }
    Ok(Redirect::to(&repo_link(&name, "issue", "").number(form.number).to_string()).into_response())
}

/// `Err` if `me` isn't `Developer`+ on `repo_name` — unlike
/// [`require_manage_permission`], no per-issue author exemption: a bulk
/// selection can span issues filed by different people, so this checks the
/// one thing that's true (or not) for the whole batch at once, same
/// `Developer`+ bar `render_issues_list`'s own checkbox column is gated on.
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

#[derive(serde::Deserialize)]
pub(crate) struct BulkIssueActionForm {
    repo: String,
    /// Checkbox values (`<input type="checkbox" name="numbers" value=...>`),
    /// repeated once per selected row — `axum`'s `Form` extractor collects
    /// every same-named field into the `Vec` the same way a multi-select
    /// `<select multiple>` would.
    #[serde(default)]
    numbers: Vec<u64>,
    /// `"close"` or `"reopen"` — anything else is rejected up front, before
    /// touching any issue.
    action: String,
    #[serde(default)]
    csrf: String,
}

/// Best-effort per issue: one bad number (already in the requested state,
/// deleted mid-flight, whatever) doesn't abort the rest of the batch — the
/// whole point of a bulk action is that most of a large selection
/// succeeding is more useful than an all-or-nothing transaction over
/// several unrelated issues. The redirect target's flash message has no
/// per-item detail either way, so a partial failure is silent by design.
pub(crate) async fn handler_bulk_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<BulkIssueActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
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
            tracker_client::close_issue(tracker, &resolved.uid, *number).await
        } else {
            tracker_client::reopen_issue(tracker, &resolved.uid, *number).await
        };
        if result.is_ok_and(|ok| ok) {
            applied += 1;
        }
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        &format!("issue.bulk_{}", form.action),
        &name,
        Some(format!("{applied}/{} issue(s)", form.numbers.len())),
    )
    .await;
    Ok(Redirect::to(&format!("/{}?view=issues", url_encode(&name))).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct EditIssueForm {
    repo: String,
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: String,
    #[serde(default)]
    assignees: String,
    /// Blank means "no milestone" — a `<select>` option with an empty
    /// value, same "empty clears it" convention `EditIssueRequest::milestone`'s
    /// own doc comment describes. A non-empty value that fails to parse
    /// (a hand-rolled POST) is treated the same as blank rather than
    /// 400ing.
    #[serde(default)]
    milestone: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_edit_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<EditIssueForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let milestone = form.milestone.trim().parse::<u64>().ok();
    let ok = tracker_client::edit_issue(
        tracker,
        &resolved.uid,
        form.number,
        &form.title,
        &form.body,
        &form.labels,
        &form.assignees,
        milestone,
    )
    .await
    .map_err(AppError::bad_request)?;
    if !ok {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "issue.edit",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "issue", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct IssueCommentForm {
    repo: String,
    number: u64,
    #[serde(default)]
    body: String,
    #[serde(default)]
    csrf: String,
}

/// No permission check beyond login, same bar as [`resolve_target`]'s own
/// doc comment on `create`.
pub(crate) async fn handler_comment_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<IssueCommentForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    tracker_client::add_issue_comment(tracker, &resolved.uid, form.number, &me.username, form.body.trim())
        .await
        .map_err(AppError::bad_request)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "issue.comment",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "issue", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct EditIssueCommentForm {
    repo: String,
    number: u64,
    comment_id: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    csrf: String,
}

/// No permission check beyond login here — tracker's own `edit_comment`
/// itself bails unless `me` is the comment's own author, same
/// author-only bar `i18n::shared::not_comment_author` describes, no
/// `Developer`+ override.
pub(crate) async fn handler_comment_edit_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<EditIssueCommentForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };

    let ok = tracker_client::edit_issue_comment(
        tracker,
        &resolved.uid,
        form.number,
        &form.comment_id,
        &me.username,
        form.body.trim(),
    )
    .await
    .map_err(AppError::bad_request)?;
    if !ok {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "issue.comment.edit",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "issue", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct DeleteIssueCommentForm {
    repo: String,
    number: u64,
    comment_id: String,
    #[serde(default)]
    csrf: String,
}

/// Author-only, unless `me` is `Developer`+ on the repo — same moderation
/// override tracker's `delete_comment`'s `is_moderator` param grants (see
/// [`handler_comment_edit_post`], which stays strictly author-only).
pub(crate) async fn handler_comment_delete_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<DeleteIssueCommentForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&form.repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &form.repo, locale).await?;
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };
    let perm =
        gitquarry_core::auth::access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &me, &name)
            .await?;
    let is_moderator = perm >= Some(Permission::Developer);

    tracker_client::delete_issue_comment(
        tracker,
        &resolved.uid,
        form.number,
        &form.comment_id,
        &me.username,
        is_moderator,
    )
    .await
    .map_err(AppError::bad_request)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "issue.comment.delete",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "issue", "").number(form.number).to_string()).into_response())
}
