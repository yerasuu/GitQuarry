//! The `/_merges*` mutation handlers the views in [`super`] POST to.
//! Top-level routes, not nested under the repo's own `/{*repo}` path — see
//! [`super`]'s own doc comment on why.

use crate::auth::web::login;
use crate::webui::repo::can_view_repo;
use crate::webui::{
    ASYNC_HEADER, AppError, State_, csrf_field, effective_density, effective_locale, effective_theme, layout,
    repo_link, require_csrf, require_login, url_encode,
};
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::merge_requests::{CommentAnchor, DiffSide, MergeStrategy, ReviewVerdict};
use gitquarry_core::validate;
use gitquarry_i18n::Locale;

/// Resolves `repo` and checks the caller can actually see it — creating a
/// MR needs only view access (see `domain::merge_request`'s own doc comment
/// on the `Reporter` bar), not `Developer`, but it does need *that*.
/// Checked here rather than inherited: these are top-level routes, not
/// nested under the repo's own `/{*repo}` path (see [`super`]'s doc
/// comment), so `handler_repo`'s own [`can_view_repo`] gate never runs in
/// front of them — without this, any session could open an MR against, or
/// comment on, a private repo it has no grant on. Close/reopen check the
/// MR's own author-or-`Developer` bar on top of this, via
/// [`require_manage_permission`] below.
///
/// A rejection is 404, not 403, matching every read path's own choice
/// (`raw::resolve_visible_repo`): a refused write must not confirm that a
/// private repo exists.
async fn resolve_repo_by_name(
    ctx: &AppContext,
    headers: &HeaderMap,
    repo: &str,
    locale: Locale,
) -> Result<(ResolvedRepo, String), AppError> {
    let name = validate::sanitize_path(repo).map_err(AppError::bad_request)?;
    let not_found = || {
        AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &name)
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

/// [`resolve_repo_by_name`] plus a check that the name still names the same
/// repo the page was rendered for: every mutating form here carries the
/// `repo_uid` its page snapshot saw, alongside the display `repo` name a
/// human can read. A repo name is a rename-able index entry (see
/// `domain::repo::rename`'s own doc comment — the old name is freed the
/// moment a rename lands, immediately claimable by an unrelated repo), so a
/// review left open across a rename can resubmit a `repo` field that now
/// resolves to *something else entirely*. Treated the same as the repo
/// simply not existing any more from that page's point of view, rather than
/// silently acting on whatever now holds the name.
async fn resolve_target(
    ctx: &AppContext,
    headers: &HeaderMap,
    repo: &str,
    expected_uid: &str,
    locale: Locale,
) -> Result<(ResolvedRepo, String), AppError> {
    let (resolved, name) = resolve_repo_by_name(ctx, headers, repo, locale).await?;
    if resolved.uid.as_str() != expected_uid {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &name)
        )));
    }
    Ok((resolved, name))
}

/// `Err` if `number` doesn't exist, or `me` is neither its author nor
/// `Developer`+ on `repo_name` — the actual state-transition rules
/// (`domain::merge_request::transition`) are checked again regardless, this
/// only gates *who* may attempt the transition at all.
async fn require_manage_permission(
    ctx: &AppContext,
    me: &User,
    target: &ResolvedRepo,
    repo_name: &str,
    number: u64,
    locale: Locale,
) -> Result<(), AppError> {
    let Some(mr) = domain::merge_request::get(ctx, target, number).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::merge_request::merge_request_not_found(locale, number)
        )));
    };
    if mr.author == me.username {
        return Ok(());
    }
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

#[derive(serde::Deserialize)]
pub(crate) struct NewMergeForm {
    repo_uid: String,
    /// Full name (`owner/name`) of the fork to propose merging from — blank
    /// means "this repo itself" (a same-repo MR). `domain::merge_request::create`
    /// is the actual gate on whether it's allowed (must be a fork of `repo`);
    /// this only resolves the name to a uid.
    #[serde(default)]
    source_repo: String,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    csrf: String,
}

/// Turns a rejected mutation on an existing MR into a redirect back to that
/// MR's page with the reason attached, rather than the generic error page —
/// which would render at the POST target, a URL that answers POST only and
/// so can't be reloaded or shared. The commonest rejections here are
/// stale-page ones (the MR was merged, its source branch is gone, someone
/// else wrote first), and the MR's own page is exactly where the reader can
/// see what actually happened.
///
/// The async path keeps the plain 400: those requests come from `app.js`
/// expecting a fragment or a status, and follow a redirect into the wrong
/// place entirely.
fn mr_error(headers: &HeaderMap, name: &str, number: u64) -> impl Fn(anyhow::Error) -> AppError {
    let back = repo_link(name, "merge", "").number(number).to_string();
    let is_async = headers.contains_key(ASYNC_HEADER);
    let headers = headers.clone();
    move |e| {
        if is_async {
            AppError::bad_request(e)
        } else {
            AppError::flash_back(&headers, &back, e)
        }
    }
}

/// `GET` on any of the `/_merges/*` mutation targets.
///
/// Those answer POST, and a rejected submit renders in the POST response,
/// so this URL is what a browser reloading or backing into the page asks
/// for. It knows which repo it belongs to — that's the point of the repo
/// being in the path — so it can land on that repo's merge requests
/// instead of the router's generic "back to the index" fallback.
pub(crate) async fn handler_mutation_get(Path(repo): Path<String>) -> Response {
    Redirect::to(&format!("/{}?view=merges", url_encode(&repo))).into_response()
}

/// [`handler_mutation_get`] for the one target whose page isn't the list:
/// a rejected create came from the new-merge-request form.
pub(crate) async fn handler_create_get(Path(repo): Path<String>) -> Response {
    Redirect::to(&format!("/{}?view=new_merge", url_encode(&repo))).into_response()
}

pub(crate) async fn handler_create_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<NewMergeForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;

    // A rejected submit (bad rev, no fork relationship, no diff, ...)
    // re-renders this same form with everything the user typed still
    // filled in, instead of losing it to the generic error page. The
    // branch suggestions are re-listed for the retry (one mirror open):
    // the commonest rejection here is "'x' isn't a branch of this repo",
    // and that sentence is only actionable next to the branches that are.
    // A mirror that won't open costs the datalist, not the retry.
    macro_rules! fail {
        ($msg:expr) => {{
            let refs = match ctx.open_mirror_read(&resolved).await {
                Ok(mirror) => gitquarry_core::git::list_refs(&mirror.path).await.unwrap_or_default(),
                // Empty because a mirror that won't open costs the datalist,
                // not the retry (see above).
                Err(_) => Vec::new(),
            };
            let ref_names: Vec<&str> = refs.iter().filter_map(|r| r.strip_prefix("refs/heads/")).collect();
            let topnav = login::topnav(&ctx, &headers).await;
            let body = super::render_new_merge_form(
                &name,
                &resolved.uid,
                &ref_names,
                None,
                super::NewMergeFormValues {
                    source_repo: &form.source_repo,
                    source_branch: &form.source_branch,
                    target_branch: &form.target_branch,
                    title: &form.title,
                    description: &form.description,
                    error: Some(&$msg),
                },
                &headers,
                locale,
            )
            .into_string();
            return Ok(Html(layout(
                "new merge request",
                "",
                &body,
                &topnav,
                effective_theme(&ctx, &headers).await,
                effective_density(&ctx, &headers).await,
                locale,
            ))
            .into_response());
        }};
    }

    let source_uid = if form.source_repo.trim().is_empty() {
        resolved.uid.clone()
    } else {
        // Freely typed, not a page snapshot -- there's no prior uid to check
        // it against, unlike every other `resolve_target` call in this file.
        let (source_resolved, _) = resolve_repo_by_name(&ctx, &headers, form.source_repo.trim(), locale).await?;
        source_resolved.uid
    };

    let source_branch = form.source_branch.trim();
    let target_branch = form.target_branch.trim();
    if let Err(e) = validate::validate_rev(source_branch) {
        fail!(e.to_string());
    }
    if let Err(e) = validate::validate_rev(target_branch) {
        fail!(e.to_string());
    }

    let new = domain::merge_request::NewMergeRequest {
        source_repo: &source_uid,
        source_branch,
        target_branch,
        title: form.title.trim(),
        description: &form.description,
        author: &me.username,
    };
    let mr = match domain::merge_request::create(&ctx, &resolved, new, locale).await {
        Ok(mr) => mr,
        Err(e) => fail!(e.to_string()),
    };

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.create",
        &format!("{name}#{}", mr.number),
        None,
    )
    .await;

    Ok(Redirect::to(&repo_link(&name, "merge", "").number(mr.number).to_string()).into_response())
}

/// Async (`data-async`/`data-async-target="#mr-actions"`, see `ASYNC_HEADER`'s
/// own doc comment) response for close/reopen/merge: the `#mr-actions`
/// fragment reflecting whatever the MR's state is *now* (re-fetched, not
/// assumed from the request that just mutated it), instead of the usual
/// redirect back to the MR page.
async fn async_mr_actions_response(
    ctx: &AppContext,
    headers: &HeaderMap,
    resolved: &ResolvedRepo,
    repo_name: &str,
    number: u64,
    locale: Locale,
) -> Result<Response, AppError> {
    let Some(mr) = domain::merge_request::get(ctx, resolved, number).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::merge_request::merge_request_not_found(locale, number)
        )));
    };
    let can_manage = super::can_manage(ctx, headers, repo_name, &mr.author).await?;
    let can_merge = super::can_merge(ctx, headers, repo_name).await?;
    let is_same_repo = mr.source_repo == resolved.uid;
    let source_branch_protected = if is_same_repo {
        let (config, _etag) = resolved.remote.get_repo_config().await?;
        config.protected_branches.contains(mr.source_branch.as_str())
    } else {
        false
    };
    // Re-probed rather than carried over from the page that rendered the
    // form: this fragment is swapped in *after* an action landed, and the
    // one that most obviously changes the answer is the merge itself.
    let fast_forwardable = if mr.status == gitquarry_core::storage::merge_requests::MrStatus::Open && can_merge {
        let mirror = ctx.open_mirror_read(resolved).await?;
        domain::merge_request::fast_forwardable(ctx, resolved, &mirror.path, &mr).await?
    } else {
        false
    };
    Ok(Html(
        super::render_mr_actions(
            &mr,
            can_manage,
            can_merge,
            is_same_repo,
            source_branch_protected,
            fast_forwardable,
            repo_name,
            &resolved.uid,
            &csrf_field(headers),
            locale,
        )
        .into_string(),
    )
    .into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct MergeActionForm {
    repo_uid: String,
    number: u64,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_close_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<MergeActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;

    let cas = domain::merge_request::close(&ctx, &resolved, form.number, locale)
        .await
        .map_err(mr_error(&headers, &name, form.number))?;
    if cas == Cas::Conflict {
        return Err(mr_error(&headers, &name, form.number)(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.close",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    if headers.contains_key(ASYNC_HEADER) {
        return async_mr_actions_response(&ctx, &headers, &resolved, &name, form.number, locale).await;
    }
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

pub(crate) async fn handler_reopen_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<MergeActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;

    let cas = domain::merge_request::reopen(&ctx, &resolved, form.number, locale)
        .await
        .map_err(mr_error(&headers, &name, form.number))?;
    if cas == Cas::Conflict {
        return Err(mr_error(&headers, &name, form.number)(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.reopen",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    if headers.contains_key(ASYNC_HEADER) {
        return async_mr_actions_response(&ctx, &headers, &resolved, &name, form.number, locale).await;
    }
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct CommentForm {
    repo_uid: String,
    number: u64,
    #[serde(default)]
    body: String,
    /// The three anchor fields the diff's inline form fills in; all absent
    /// on the plain thread form at the bottom of the page.
    #[serde(default)]
    anchor_path: String,
    #[serde(default)]
    anchor_side: String,
    #[serde(default)]
    anchor_line: u32,
    #[serde(default)]
    csrf: String,
}

/// The submitted anchor, or `None` for a plain thread comment. The path
/// goes through [`validate::validate_tree_path`] like every other
/// network-supplied repo path, even though an anchor is only ever compared
/// against diff text — it's stored and rendered back, so it gets the same
/// gate.
fn parse_anchor(form: &CommentForm) -> Result<Option<CommentAnchor>, AppError> {
    if form.anchor_path.is_empty() && form.anchor_line == 0 {
        return Ok(None);
    }
    validate::validate_tree_path(&form.anchor_path).map_err(AppError::bad_request)?;
    Ok(Some(CommentAnchor {
        path: form.anchor_path.clone(),
        side: if form.anchor_side == "old" {
            DiffSide::Old
        } else {
            DiffSide::New
        },
        line: form.anchor_line,
    }))
}

/// No permission check beyond login, same as [`resolve_target`]'s own doc
/// comment on `create` — commenting needs only the `Reporter`-tier view
/// access `handler_repo` already gated the whole page behind.
pub(crate) async fn handler_comment_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<CommentForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;

    let body = form.body.trim();
    match parse_anchor(&form)? {
        Some(anchor) => {
            domain::merge_request::add_line_comment(&ctx, &resolved, form.number, &me.username, body, anchor, locale)
                .await
        }
        None => domain::merge_request::add_comment(&ctx, &resolved, form.number, &me.username, body, locale).await,
    }
    .map_err(mr_error(&headers, &name, form.number))?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.comment",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct EditMrCommentForm {
    repo_uid: String,
    number: u64,
    comment_id: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    csrf: String,
}

/// No permission check beyond login here — `domain::merge_request::edit_comment`
/// itself bails unless `me` is the comment's own author, no `Developer`+
/// override (same bar `issues::handlers::handler_comment_edit_post` has).
pub(crate) async fn handler_comment_edit_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<EditMrCommentForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;

    let cas = domain::merge_request::edit_comment(
        &ctx,
        &resolved,
        form.number,
        &form.comment_id,
        &me.username,
        form.body.trim(),
        locale,
    )
    .await
    .map_err(mr_error(&headers, &name, form.number))?;
    if cas == Cas::Conflict {
        return Err(mr_error(&headers, &name, form.number)(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.comment.edit",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct DeleteMrCommentForm {
    repo_uid: String,
    number: u64,
    comment_id: String,
    #[serde(default)]
    csrf: String,
}

/// Author-only, unless `me` is `Developer`+ on the repo — same moderation
/// override `domain::merge_request::delete_comment`'s `is_moderator` param
/// grants (see [`handler_comment_edit_post`], which stays strictly
/// author-only).
pub(crate) async fn handler_comment_delete_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<DeleteMrCommentForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    let perm =
        gitquarry_core::auth::access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &me, &name)
            .await?;
    let is_moderator = perm >= Some(Permission::Developer);

    domain::merge_request::delete_comment(
        &ctx,
        &resolved,
        form.number,
        &form.comment_id,
        &me.username,
        is_moderator,
        locale,
    )
    .await
    .map_err(mr_error(&headers, &name, form.number))?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.comment.delete",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct ReviewForm {
    repo_uid: String,
    number: u64,
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    csrf: String,
}

/// The form's `verdict` select value. Anything unrecognized (a hand-rolled
/// POST) falls back to the neutral verdict rather than 400ing: the worst
/// case is a review that says less than intended, never one that approves
/// something it shouldn't.
pub(super) fn parse_verdict(raw: &str) -> ReviewVerdict {
    match raw {
        "approved" => ReviewVerdict::Approved,
        "changes_requested" => ReviewVerdict::ChangesRequested,
        _ => ReviewVerdict::Commented,
    }
}

/// No permission check beyond login, same bar as [`handler_comment_post`] —
/// a review is a signal, not a write to the repo. The one rule
/// (`domain::merge_request::add_review`: no approving your own MR) lives in
/// the domain layer, checked there regardless of what this form offered.
pub(crate) async fn handler_review_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ReviewForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;

    let verdict = parse_verdict(form.verdict.trim());
    domain::merge_request::add_review(
        &ctx,
        &resolved,
        form.number,
        &me.username,
        verdict,
        form.body.trim(),
        locale,
    )
    .await
    .map_err(mr_error(&headers, &name, form.number))?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.review",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

/// `Err` if `me` isn't at least `Developer` on `repo_name` — unlike
/// [`require_manage_permission`] (close/reopen: author-or-`Developer`),
/// merging is `Developer`+ only, no author exemption: opening an MR needs
/// only view access, but landing it on the target branch is a write to
/// that branch, same bar every other write to this repo already has.
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

#[derive(serde::Deserialize)]
pub(crate) struct MergeForm {
    repo_uid: String,
    number: u64,
    #[serde(default)]
    strategy: MergeStrategy,
    /// `"1"` when the checkbox is checked, absent (empty string) otherwise
    /// — same convention `repo_settings::general`'s own `archived`
    /// checkbox field already uses.
    #[serde(default)]
    delete_branch: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_merge_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<MergeForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    require_developer_permission(&ctx, &me, &name, locale).await?;

    // Fetched before merging (not after — `merge` doesn't touch
    // `source_repo`/`source_branch`, but reading it now means one fewer
    // storage round-trip either way) so the branch-delete below has
    // something to act on. Only a same-repo MR's source branch is ever
    // eligible — a cross-fork MR's source branch lives on a different
    // repo entirely, out of scope for this checkbox.
    let source_branch = domain::merge_request::get(&ctx, &resolved, form.number)
        .await
        .map_err(mr_error(&headers, &name, form.number))?
        .filter(|mr| mr.source_repo == resolved.uid)
        .map(|mr| mr.source_branch);

    match domain::merge_request::merge(&ctx, &resolved, form.number, &me.username, form.strategy, locale)
        .await
        .map_err(mr_error(&headers, &name, form.number))?
    {
        domain::merge_request::MergeOutcome::Merged(sha) => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &me.username,
                "merge_request.merge",
                &format!("{name}#{}", form.number),
                Some(sha),
            )
            .await;
            // Best-effort, same tolerance the merge's own closing-keyword
            // auto-close has: a protected or already-gone branch just
            // silently doesn't delete rather than failing the merge that
            // already succeeded.
            if form.delete_branch == "1"
                && let Some(branch) = source_branch
            {
                let _ = domain::repo::delete_branch(&resolved, &branch, locale).await;
            }
        }
        domain::merge_request::MergeOutcome::Conflict(paths) => {
            // An async submit swaps `#mr-actions` with whatever comes back,
            // so a redirect's full page would land inside it; a real error
            // status instead reaches `style/app.js`'s toast, and the notice
            // with the paths renders on the next load of the MR page.
            if headers.contains_key(ASYNC_HEADER) {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}: {}",
                    gitquarry_i18n::web::repo::merges::conflicts_heading(locale),
                    paths.join(", ")
                )));
            }
            // Back to the MR, which now renders the conflicting files and
            // how to resolve them (`merges::render_conflict_notice`) — the
            // `400` this used to return put that same list on a page the
            // reader had to leave, and losing it on reload.
            return Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response());
        }
    }
    if headers.contains_key(ASYNC_HEADER) {
        return async_mr_actions_response(&ctx, &headers, &resolved, &name, form.number, locale).await;
    }
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct EditPrMetadataForm {
    repo_uid: String,
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    labels: String,
    #[serde(default)]
    assignees: String,
    #[serde(default)]
    reviewers: String,
    #[serde(default)]
    csrf: String,
}

/// Same author-or-`Developer` bar as close/reopen — editing an MR's
/// title/description/labels/assignees/reviewers is metadata management,
/// not a write to the repo itself, but still not something a bare
/// `Reporter` viewer should be able to do to someone else's MR.
/// `domain::merge_request::edit_metadata` also rejects a non-`Open` MR —
/// closed/merged MRs aren't editable even by the author or a maintainer.
pub(crate) async fn handler_edit_metadata_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<EditPrMetadataForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;

    let cas = domain::merge_request::edit_metadata(
        &ctx,
        &resolved,
        form.number,
        domain::merge_request::MrMetadataEdit {
            title: &form.title,
            description: &form.description,
            labels_csv: &form.labels,
            assignees_csv: &form.assignees,
            reviewers_csv: &form.reviewers,
        },
        locale,
    )
    .await
    .map_err(mr_error(&headers, &name, form.number))?;
    if cas == Cas::Conflict {
        return Err(mr_error(&headers, &name, form.number)(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.edit_metadata",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

pub(crate) async fn handler_mark_draft_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<MergeActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;

    let cas = domain::merge_request::mark_draft(&ctx, &resolved, form.number, locale)
        .await
        .map_err(mr_error(&headers, &name, form.number))?;
    if cas == Cas::Conflict {
        return Err(mr_error(&headers, &name, form.number)(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.mark_draft",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    if headers.contains_key(ASYNC_HEADER) {
        return async_mr_actions_response(&ctx, &headers, &resolved, &name, form.number, locale).await;
    }
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

pub(crate) async fn handler_mark_ready_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<MergeActionForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    require_manage_permission(&ctx, &me, &resolved, &name, form.number, locale).await?;

    let cas = domain::merge_request::mark_ready(&ctx, &resolved, form.number, locale)
        .await
        .map_err(mr_error(&headers, &name, form.number))?;
    if cas == Cas::Conflict {
        return Err(mr_error(&headers, &name, form.number)(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "merge_request.mark_ready",
        &format!("{name}#{}", form.number),
        None,
    )
    .await;
    if headers.contains_key(ASYNC_HEADER) {
        return async_mr_actions_response(&ctx, &headers, &resolved, &name, form.number, locale).await;
    }
    Ok(Redirect::to(&repo_link(&name, "merge", "").number(form.number).to_string()).into_response())
}

/// Read by hand from the raw body rather than through axum's `Form`:
/// `serde_urlencoded` can't collect a key repeated once per checked row into
/// a `Vec`, so every submission with a selection was a 422.
#[derive(Default)]
struct BulkPrActionForm {
    repo_uid: String,
    /// Checkbox values (`<input type="checkbox" name="numbers" value=...>`),
    /// one per selected row. A value that isn't a number names no MR, so it
    /// is dropped rather than failing the whole batch.
    numbers: Vec<u64>,
    /// `"close"` or `"reopen"` — anything else is rejected up front, before
    /// touching any MR.
    action: String,
    csrf: String,
}

impl BulkPrActionForm {
    fn parse(body: &[u8]) -> Self {
        let mut form = Self::default();
        for (key, value) in form_urlencoded::parse(body) {
            match key.as_ref() {
                "repo_uid" => form.repo_uid = value.into_owned(),
                "numbers" => form.numbers.extend(value.parse::<u64>().ok()),
                "action" => form.action = value.into_owned(),
                "csrf" => form.csrf = value.into_owned(),
                _ => {}
            }
        }
        form
    }
}

/// Best-effort per MR, same tolerance `issues::handlers::handler_bulk_post`
/// already accepts: one already-merged or already-in-that-state MR in the
/// selection doesn't abort the rest of the batch (`domain::merge_request::close`/
/// `reopen` just errors for that one number, which this loop ignores).
pub(crate) async fn handler_bulk_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let form = BulkPrActionForm::parse(&body);
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = resolve_target(&ctx, &headers, &repo, &form.repo_uid, locale).await?;
    require_developer_permission(&ctx, &me, &name, locale).await?;

    if !matches!(form.action.as_str(), "close" | "reopen") {
        // Back to the list this batch was selected on -- there's no single
        // MR page to return to here.
        return Err(AppError::flash_back(
            &headers,
            &format!("/{}?view=merges", url_encode(&name)),
            anyhow::anyhow!("{}", gitquarry_i18n::shared::unknown_action(locale, &form.action)),
        ));
    }
    let mut applied = 0usize;
    for number in &form.numbers {
        let result = if form.action == "close" {
            domain::merge_request::close(&ctx, &resolved, *number, locale).await
        } else {
            domain::merge_request::reopen(&ctx, &resolved, *number, locale).await
        };
        if result.is_ok() {
            applied += 1;
        }
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        &format!("merge_request.bulk_{}", form.action),
        &name,
        Some(format!("{applied}/{} merge request(s)", form.numbers.len())),
    )
    .await;
    Ok(Redirect::to(&format!("/{}?view=merges", url_encode(&name))).into_response())
}
