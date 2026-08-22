//! The `merges`/`merge`/`new_merge` views (merge requests, same-repo or
//! cross-fork — see `domain::merge_request`'s own doc comment). The
//! `/_merges*` mutation handlers those views' forms POST to live in
//! [`handlers`]: top-level routes, not nested under the repo's own
//! `/{*repo}` path — same shape as `/_releases` (`super::releases`'s doc
//! comment explains why: that path is a full-path catch-all already bound
//! to one POST handler, so a second mutation action set needs its own
//! literal route, taking the repo's full name as a hidden form field
//! instead of a path segment).

use super::components::{commit_table, side_by_side_diff};
use crate::auth::web::login;
use crate::webui::{
    AppError, DiffView, csrf_field, effective_diff_view, render_diff_view_buttons, repo_link, url_encode,
};
use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::domain;
use gitquarry_core::storage::merge_requests::{MrReview, MrStatus, ReviewVerdict};
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo_merges as t;
use maud::{Markup, PreEscaped, html};

fn status_label(status: MrStatus, locale: Locale) -> String {
    match status {
        MrStatus::Open => t::open_status(locale),
        MrStatus::Closed => t::closed_status(locale),
        MrStatus::Merged => t::merged_status(locale),
    }
}

/// `styler` badge variant for the merges-list status pill: open is
/// `.badge.ok`, merged is `.badge.info` (`--info` is already aliased to
/// this project's own `--link` blue, see `light.css`/`dark.css`), closed
/// stays the neutral bare `.badge`.
fn status_badge_class(status: MrStatus) -> &'static str {
    match status {
        MrStatus::Open => "badge ok",
        MrStatus::Closed => "badge",
        MrStatus::Merged => "badge info",
    }
}

/// Status line + close/reopen/merge buttons + merged-by paragraph, factored
/// out of [`render_merge_detail`] so the close/reopen/merge handlers can
/// return just this fragment (wrapped in `#mr-actions`, see the `data-async`
/// forms below) after an async submit — closing an MR changes all of this
/// at once (status text, which button shows next, whether the merged-by
/// line appears), not just one button's own label the way star/watch/theme
/// do, so the whole block has to refresh together.
/// What to show when the MR can't fast-forward cleanly: which files
/// collide, and the exact commands to resolve it locally.
///
/// Everything here used to be a `400` page thrown by the merge handler,
/// which meant the one moment a reader most needs the file list is the one
/// moment they get taken off the page — and a reload lost it entirely.
/// It's part of the MR now, visible before anyone presses merge.
///
/// The instructions are deliberately the plain git ones rather than a
/// button: this web UI never writes repository content (see
/// `webui`'s own module doc), so resolving a conflict is a local
/// operation, and pretending otherwise would be the one place the UI
/// promised an edit it can't perform.
fn render_conflict_notice(
    repo_name: &str,
    mr: &gitquarry_core::storage::merge_requests::MergeRequestMeta,
    paths: &[String],
    locale: Locale,
) -> Markup {
    html! {
        div class="alert alert-warn" {
            span class="alert-body" {
                strong { (t::conflicts_heading(locale)) }
                " " (t::conflicts_intro(locale, mr.target_branch.as_str()))
            }
        }
        ul class="conflict-paths" {
            @for path in paths {
                li { code { (path) } }
            }
        }
        details class="conflict-help" {
            summary { (t::conflicts_howto_summary(locale)) }
            pre { (conflict_commands(repo_name, mr)) }
        }
    }
}

/// The copy-pasteable resolution sequence. `repo_name` only appears in a
/// comment line, never interpolated into a shell word that could run —
/// nothing here is executed by the server, but a repo name reaching a
/// `pre` block is still untrusted text, and `maud` escapes it on the way
/// out.
fn conflict_commands(repo_name: &str, mr: &gitquarry_core::storage::merge_requests::MergeRequestMeta) -> String {
    format!(
        "# {repo_name}\n\
         git fetch origin\n\
         git switch {source}\n\
         git merge origin/{target}\n\
         # resolve the files listed above, then:\n\
         git add -A && git commit\n\
         git push origin {source}\n",
        source = mr.source_branch,
        target = mr.target_branch,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_mr_actions(
    mr: &gitquarry_core::storage::merge_requests::MergeRequestMeta,
    can_manage: bool,
    can_merge: bool,
    is_same_repo: bool,
    fast_forwardable: bool,
    repo_name: &str,
    csrf_html: &str,
    locale: Locale,
) -> Markup {
    html! {
        div id="mr-actions" {
            p class="muted" {
                (status_label(mr.status, locale))
                @if mr.is_draft && mr.status == MrStatus::Open {
                    " " span class="badge warn" { (t::draft_badge(locale)) }
                }
            }
            @if can_manage {
                @match mr.status {
                    MrStatus::Open => {
                        form method="post" action="/_merges/close" style="display:inline" data-async data-async-target="#mr-actions" {
                            (PreEscaped(csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="number" value=(mr.number);
                            button type="submit" { (t::close_button(locale)) }
                        }
                        " "
                        @if mr.is_draft {
                            form method="post" action="/_merges/mark_ready" style="display:inline" data-async data-async-target="#mr-actions" {
                                (PreEscaped(csrf_html))
                                input type="hidden" name="repo" value=(repo_name);
                                input type="hidden" name="number" value=(mr.number);
                                button type="submit" { (t::mark_ready_button(locale)) }
                            }
                        } @else {
                            form method="post" action="/_merges/mark_draft" style="display:inline" data-async data-async-target="#mr-actions" {
                                (PreEscaped(csrf_html))
                                input type="hidden" name="repo" value=(repo_name);
                                input type="hidden" name="number" value=(mr.number);
                                button type="submit" { (t::mark_draft_button(locale)) }
                            }
                        }
                    }
                    MrStatus::Closed => {
                        form method="post" action="/_merges/reopen" style="display:inline" data-async data-async-target="#mr-actions" {
                            (PreEscaped(csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="number" value=(mr.number);
                            button type="submit" { (t::reopen_button(locale)) }
                        }
                    }
                    MrStatus::Merged => {}
                }
            }
            @if can_merge && !mr.is_draft && mr.status == MrStatus::Open {
                form method="post" action="/_merges/merge" style="display:inline" data-async data-async-target="#mr-actions" data-confirm=(t::merge_confirm(locale)) {
                    (PreEscaped(csrf_html))
                    input type="hidden" name="repo" value=(repo_name);
                    input type="hidden" name="number" value=(mr.number);
                    label for="merge-strategy" class="muted" { (t::merge_strategy_label(locale)) }
                    select id="merge-strategy" name="strategy" {
                        option value="Merge" { (t::merge_strategy_merge_option(locale)) }
                        option value="Squash" { (t::merge_strategy_squash_option(locale)) }
                        option value="Rebase" { (t::merge_strategy_rebase_option(locale)) }
                        // Disabled rather than hidden when the branches have
                        // diverged: an option that silently disappears reads
                        // as "this forge can't do that", where a greyed one
                        // says "not for this merge request, right now".
                        option value="FastForward" disabled[!fast_forwardable] {
                            (t::merge_strategy_fast_forward_option(locale))
                            @if !fast_forwardable { " " (t::merge_strategy_fast_forward_unavailable(locale)) }
                        }
                    }
                    @if is_same_repo {
                        label {
                            input type="checkbox" name="delete_branch" value="1";
                            " " (t::delete_branch_checkbox_label(locale))
                        }
                    }
                    button type="submit" { (t::merge_button(locale)) }
                }
            }
            @if let MrStatus::Merged = mr.status {
                p class="muted" {
                    (t::merged_by_prefix(locale))
                    (mr.merged_by.as_deref().unwrap_or(""))
                    (t::merged_by_middle(locale))
                    (mr.merged_at.as_deref().unwrap_or(""))
                }
                @if let Some(sha) = &mr.merge_commit_sha {
                    p class="muted" { (t::merge_commit_prefix(locale)) code { (gitquarry_core::validate::short_sha(sha)) } }
                }
            }
        }
    }
}

fn verdict_label(verdict: ReviewVerdict, locale: Locale) -> String {
    match verdict {
        ReviewVerdict::Approved => t::approved_verdict(locale),
        ReviewVerdict::ChangesRequested => t::changes_requested_verdict(locale),
        ReviewVerdict::Commented => t::commented_verdict(locale),
    }
}

/// The review thread plus its collapsed [`domain::merge_request::review_summary`]
/// state, and — for a logged-in viewer — the submit form. The verdict
/// options offered depend on whether the viewer authored the MR: an author
/// can only leave a neutral review (`domain::merge_request::add_review`
/// rejects anything else regardless of what this form offered).
/// One MR body -- a description, a thread comment, a review, an inline
/// diff note -- as HTML. They are all the same kind of untrusted markdown
/// written by the same people in the same box, so they go through the same
/// two steps: resolve the `#123` mentions, then render.
async fn render_body(ctx: &AppContext, target: &ResolvedRepo, repo_name: &str, body: &str) -> String {
    let mentions = super::components::resolve_number_mentions(ctx, target, repo_name, body).await;
    crate::webui::markdown::render_with_mentions(body, &mentions)
}

/// `bodies` holds one pre-rendered body per entry of `reviews`, in the same
/// order -- rendering one is async (it resolves `#123` mentions against
/// storage) and this isn't.
#[allow(clippy::too_many_arguments)]
fn render_reviews(
    reviews: &[MrReview],
    bodies: &[String],
    repo_name: &str,
    number: u64,
    viewer: Option<&str>,
    mr_author: &str,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    let summary = domain::merge_request::review_summary(reviews);
    let is_own_mr = viewer == Some(mr_author);

    html! {
        h4 { (t::reviews_heading(locale)) }
        @if !summary.approved_by.is_empty() {
            p class="muted" { (t::approved_by_prefix(locale)) (summary.approved_by.join(", ")) }
        }
        @if !summary.changes_requested_by.is_empty() {
            p class="muted" { (t::changes_requested_by_prefix(locale)) (summary.changes_requested_by.join(", ")) }
        }
        @if reviews.is_empty() {
            p class="muted" { (t::no_reviews_hint(locale)) }
        }
        @for (i, review) in reviews.iter().enumerate() {
            div class="commit-meta" {
                p class="muted" {
                    strong { (review.author) } " " (verdict_label(review.verdict, locale)) " — " (review.created_at)
                }
                @if !review.body.is_empty() {
                    div class="readme plain" { (PreEscaped(&bodies[i])) }
                }
            }
        }
        @if viewer.is_some() {
            form method="post" action="/_merges/review" {
                (PreEscaped(csrf_field(headers)))
                input type="hidden" name="repo" value=(repo_name);
                input type="hidden" name="number" value=(number);
                @if is_own_mr {
                    p class="muted" { (t::own_merge_request_review_hint(locale)) }
                    input type="hidden" name="verdict" value="commented";
                } @else {
                    p {
                        label { (t::review_verdict_label(locale)) " " }
                        select name="verdict" {
                            option value="commented" { (t::commented_verdict(locale)) }
                            option value="approved" { (t::approved_verdict(locale)) }
                            option value="changes_requested" { (t::changes_requested_verdict(locale)) }
                        }
                    }
                }
                textarea name="body" rows="3" cols="60" {}
                p { button type="submit" { (t::submit_review_button(locale)) } }
            }
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub async fn render_merges_list(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    status: &str,
    q: Option<&str>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    render_merges_list_impl(ctx, target, repo_name, status, q, headers, locale).await
}
#[cfg(not(any(test, feature = "test-util")))]
pub(super) async fn render_merges_list(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    status: &str,
    q: Option<&str>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    render_merges_list_impl(ctx, target, repo_name, status, q, headers, locale).await
}

/// Same shape as `issues::status_tabs` — a "closed" tab intentionally
/// includes `Merged` too (matches GitHub's own MR list: the "closed" tab
/// shows merged MRs with a purple badge rather than splitting into a third
/// tab), so it maps to "not open" here, not `MrStatus::Closed` alone.
fn status_tabs(repo_name: &str, status: &str, q: Option<&str>, locale: Locale) -> Markup {
    let q_suffix = q.map(|q| format!("&q={}", url_encode(q))).unwrap_or_default();
    let tabs: Vec<(&str, String, String)> = [
        ("open", t::open_status(locale)),
        ("closed", t::closed_status(locale)),
        ("all", t::all_status_filter(locale)),
    ]
    .into_iter()
    .map(|(id, label)| {
        (
            id,
            format!("/{}?view=merges&status={id}{q_suffix}", url_encode(repo_name)),
            label,
        )
    })
    .collect();
    html! {
        // `.subnav`: stacks directly under the main tabs bar's own row --
        // see `base.css`'s own doc comment on `table.tabs.subnav`. No
        // hamburger toggle (`tab_links`'s own `toggle: None`) -- only 3
        // entries, already reads fine as a short always-visible stack on
        // mobile.
        table class="tabs subnav" {
            tr {
                td { (crate::webui::tab_links(None, &tabs, status)) }
            }
        }
    }
}

/// `status` narrows to `"open"`/`"closed"` (the latter meaning "not open",
/// see [`status_tabs`]), anything else (including `"all"`) shows every MR.
/// `q`, when given, further keeps only MRs whose title or description
/// contains it (case-insensitive) — same substring-`retain()` shape
/// `issues::render_issues_list`'s own `q` filter already uses.
async fn render_merges_list_impl(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    status: &str,
    q: Option<&str>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let mut mrs = domain::merge_request::list(ctx, target, None).await?;
    match status {
        "open" => mrs.retain(|mr| mr.status == MrStatus::Open),
        "closed" => mrs.retain(|mr| mr.status != MrStatus::Open),
        _ => {}
    }
    if let Some(q) = q {
        let q_lower = q.to_ascii_lowercase();
        mrs.retain(|mr| {
            mr.title.to_ascii_lowercase().contains(&q_lower) || mr.description.to_ascii_lowercase().contains(&q_lower)
        });
    }
    let avatars = gitquarry_core::auth::access::avatar_content_types(
        &ctx.backend(),
        &ctx.cipher,
        mrs.iter().map(|mr| mr.author.clone()),
    )
    .await;
    let logged_in = login::current_user(ctx, headers).await.is_some();
    // Same `Developer`+ bar `can_merge` already gates a single MR's merge
    // button on — a bulk selection can span MRs opened by different
    // authors, so this checks the one thing true (or not) for the whole
    // batch, same reasoning `issues::require_developer_permission` uses.
    let can_bulk_manage = can_merge(ctx, headers, repo_name).await?;
    let narrowed = q.is_some() || status != "open";
    let no_matches_hint = if narrowed {
        t::no_matches_hint(locale)
    } else {
        t::no_merges_yet_hint(locale)
    };
    let csrf_html = csrf_field(headers);

    Ok(html! {
        (status_tabs(repo_name, status, q, locale))
        form method="get" action=(format!("/{}", url_encode(repo_name))) {
            input type="hidden" name="view" value="merges";
            input type="hidden" name="status" value=(status);
            input name="q" value=(q.unwrap_or("")) placeholder=(t::search_merges_placeholder(locale));
            button type="submit" { (t::search_button(locale)) }
        }
        @if can_bulk_manage {
            form method="post" action="/_merges/bulk" {
                (PreEscaped(&csrf_html))
                input type="hidden" name="repo" value=(repo_name);
                div class="table-scroll" {
                table class="list" {
                    tr {
                        th {}
                        th { (t::number_column(locale)) } th { (t::title_column(locale)) } th { (t::status_column(locale)) } th { (t::author_column(locale)) }
                    }
                    @if mrs.is_empty() {
                        tr { td colspan="5" { div class="empty-state" { div class="empty-state-title" { (no_matches_hint) } } } }
                    }
                    @for mr in &mrs {
                        tr {
                            td { @if mr.status != MrStatus::Merged { input type="checkbox" name="numbers" value=(mr.number); } }
                            td { (mr.number) }
                            td { a href=(repo_link(repo_name, "merge", "").number(mr.number).to_string()) { (mr.title) } }
                            td { span class=(status_badge_class(mr.status)) { (status_label(mr.status, locale)) } }
                            td { (crate::webui::identicon::account_avatar(&mr.author, avatars.get(&mr.author).map(String::as_str))) " " (mr.author) }
                        }
                    }
                }
                }
                @if !mrs.is_empty() {
                    p {
                        select name="action" {
                            option value="close" { (t::bulk_close_option(locale)) }
                            option value="reopen" { (t::bulk_reopen_option(locale)) }
                        }
                        button type="submit" { (t::bulk_apply_button(locale)) }
                    }
                }
            }
        } @else {
            div class="table-scroll" {
            table class="list" {
                tr { th { (t::number_column(locale)) } th { (t::title_column(locale)) } th { (t::status_column(locale)) } th { (t::author_column(locale)) } }
                @if mrs.is_empty() {
                    tr { td colspan="4" { div class="empty-state" { div class="empty-state-title" { (no_matches_hint) } } } }
                }
                @for mr in &mrs {
                    tr {
                        td { (mr.number) }
                        td { a href=(repo_link(repo_name, "merge", "").number(mr.number).to_string()) { (mr.title) } }
                        td { span class=(status_badge_class(mr.status)) { (status_label(mr.status, locale)) } }
                        td { (crate::webui::identicon::account_avatar(&mr.author, avatars.get(&mr.author).map(String::as_str))) " " (mr.author) }
                    }
                }
            }
            }
        }
        // `.btn` on a plain `<a>`: a GET navigation, not a mutation.
        @if logged_in {
            p { a class="btn" href=(format!("/{}?view=new_merge", url_encode(repo_name))) { (t::new_merge_button(locale)) } }
        }
    }
    .into_string())
}

/// A GET form (linkable/bookmarkable, unlike the settings tab's POST forms) —
/// same shape as `compare::render_compare_form`, with every branch as options
/// on both source and target, free text also works (`validate_rev` at the
/// handler is the real gate).
/// `template` is `MERGE_REQUEST_TEMPLATE.md`'s content from the repo's
/// default branch, if the file exists — pre-fills the description textarea
/// the same way GitHub's own MR template does, editable/clearable before
/// submit like any other pre-filled form field, never enforced at the
/// handler.
/// Values to redisplay after a rejected `POST /_merges` (bad rev, no diff,
/// ...) — all `""`/`None` for the plain GET landing, same
/// [`render_new_merge_form`] either way so a failure just shows the reason
/// next to the still-filled-in form instead of losing it.
pub(super) struct NewMergeFormValues<'a> {
    pub(super) source_repo: &'a str,
    pub(super) source_branch: &'a str,
    pub(super) target_branch: &'a str,
    pub(super) title: &'a str,
    pub(super) description: &'a str,
    pub(super) error: Option<&'a str>,
}

pub(super) fn render_new_merge_form(
    repo_name: &str,
    ref_names: &[&str],
    template: Option<&str>,
    values: NewMergeFormValues<'_>,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    let description = if values.description.is_empty() {
        template.unwrap_or("")
    } else {
        values.description
    };
    html! {
        h3 { (t::new_merge_heading(locale)) }
        @if let Some(msg) = values.error {
            p class="alert alert-err" { (msg) }
        }
        form method="post" action="/_merges" {
            (PreEscaped(csrf_field(headers)))
            input type="hidden" name="repo" value=(repo_name);
            div class="field" {
                label { (t::source_repo_label(locale)) }
                input name="source_repo" size="30" placeholder=(t::source_repo_placeholder(locale)) value=(values.source_repo);
            }
            div class="field" {
                label { (t::source_branch_label(locale)) }
                input list="merge-refs" name="source_branch" size="30" value=(values.source_branch);
            }
            div class="field" {
                label { (t::target_branch_label(locale)) }
                input list="merge-refs" name="target_branch" size="30" value=(values.target_branch);
            }
            div class="field" {
                label { (t::title_label(locale)) }
                input name="title" size="40" value=(values.title);
            }
            div class="field" {
                label { (t::description_label(locale)) }
                textarea name="description" rows="4" cols="60" { (description) }
            }
            datalist id="merge-refs" {
                @for name in ref_names {
                    option value=(name) { (name) }
                }
            }
            p { button type="submit" { (t::create_merge_button(locale)) } }
        }
    }
}

/// `true` if the current session can close/reopen this MR: its own author,
/// or `Developer`+ on the repo — the actual mutation handlers below
/// re-check this themselves rather than trusting that a form was only ever
/// shown to someone entitled to submit it.
async fn can_manage(ctx: &AppContext, headers: &HeaderMap, repo_name: &str, author: &str) -> Result<bool, AppError> {
    let Some(user) = login::current_user(ctx, headers).await else {
        return Ok(false);
    };
    if user.username == author {
        return Ok(true);
    }
    let perm = gitquarry_core::auth::access::effective_permission_for_user(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &user,
        repo_name,
    )
    .await?;
    Ok(perm >= Some(Permission::Developer))
}

/// `true` if the current session can merge this MR: `Developer`+ on the
/// repo, no author exemption — opening an MR only needs view access, but
/// landing it on the target branch is a write to that branch, same bar
/// every other write to this repo already has.
async fn can_merge(ctx: &AppContext, headers: &HeaderMap, repo_name: &str) -> Result<bool, AppError> {
    let Some(user) = login::current_user(ctx, headers).await else {
        return Ok(false);
    };
    let perm = gitquarry_core::auth::access::effective_permission_for_user(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &user,
        repo_name,
    )
    .await?;
    Ok(perm >= Some(Permission::Developer))
}

/// Same `#[allow]` every other multi-source webui view carries
/// (`remote_merges::render_remote_merge_detail`, `repo::handler_repo`): the
/// params are all genuinely independent request inputs, and bundling them
/// into a struct only to unpack it at the single call site buys nothing.
#[allow(clippy::too_many_arguments)]
#[cfg(any(test, feature = "test-util"))]
pub async fn render_merge_detail(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    git_dir: &std::path::Path,
    number: u64,
    params: &std::collections::HashMap<String, String>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    render_merge_detail_impl(ctx, target, repo_name, git_dir, number, params, headers, locale).await
}
#[allow(clippy::too_many_arguments)]
#[cfg(not(any(test, feature = "test-util")))]
pub(super) async fn render_merge_detail(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    git_dir: &std::path::Path,
    number: u64,
    params: &std::collections::HashMap<String, String>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    render_merge_detail_impl(ctx, target, repo_name, git_dir, number, params, headers, locale).await
}

#[allow(clippy::too_many_arguments)]
async fn render_merge_detail_impl(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    git_dir: &std::path::Path,
    number: u64,
    params: &std::collections::HashMap<String, String>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let Some(mr) = domain::merge_request::get(ctx, target, number).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::merge_request_not_found(locale, number)
        )));
    };
    let can_manage = can_manage(ctx, headers, repo_name, &mr.author).await?;
    let can_merge = can_merge(ctx, headers, repo_name).await?;
    let (target_config, _etag) = target.remote.get_repo_config().await?;
    let label_colors: std::collections::HashMap<&str, &str> = target_config
        .labels
        .iter()
        .map(|l| (l.name.as_str(), l.color.as_str()))
        .collect();
    let d = domain::merge_request::diff(ctx, target, git_dir, &mr).await?;
    // Only for an open MR anyone may merge: on a closed one the whole merge
    // form is absent, so the probe would be a `git merge-base` spawn for an
    // answer nothing renders.
    let fast_forwardable = if mr.status == MrStatus::Open && can_merge {
        domain::merge_request::fast_forwardable(ctx, target, git_dir, &mr).await?
    } else {
        false
    };
    // Only for an open MR: a merged or closed one has nothing left to
    // merge, so the probe would be a `git merge-tree` spawn per render for
    // an answer nobody can act on.
    let conflicts = if mr.status == MrStatus::Open {
        domain::merge_request::conflicts(ctx, target, git_dir, &mr).await?
    } else {
        None
    };
    let commit_avatars = super::components::commit_avatars(ctx, git_dir, &d.commits).await;
    let comments = domain::merge_request::list_comments(ctx, target, number).await?;
    let reviews = domain::merge_request::list_reviews(ctx, target, number).await?;
    let viewer = login::current_user(ctx, headers).await.map(|u| u.username);
    let logged_in = viewer.is_some();
    let diff_view = effective_diff_view(headers);
    let return_to = repo_link(repo_name, "merge", "").number(mr.number).to_string();

    // Only the unified view can place a comment on a line (see
    // `annotated_diff`'s own doc comment) — in the split view every anchored
    // comment stays unplaced and shows up in the flat thread below instead.
    // Rendered up front, and for the whole comment list rather than just the
    // anchored ones: which comments the diff actually places is decided
    // inside `annotated_diff::render`, and it indexes into this by the same
    // position, so the two have to line up one-for-one.
    let mut comment_bodies = Vec::with_capacity(comments.len());
    for (_id, comment) in &comments {
        comment_bodies.push(render_body(ctx, target, repo_name, &comment.body).await);
    }

    let active_anchor = annotated_diff::active_anchor(params);
    let annotated = match diff_view {
        DiffView::Unified => Some(annotated_diff::render(
            &d.diff,
            &comments,
            &annotated_diff::View {
                comment_bodies: &comment_bodies,
                repo_name,
                number: mr.number,
                can_comment: logged_in,
                active: active_anchor.as_ref(),
                viewer: viewer.as_deref(),
                can_moderate: can_merge,
                headers,
                locale,
            },
        )),
        DiffView::Split => None,
    };
    let placed = annotated.as_ref().map(|a| &a.placed);
    // Kept as indexes, not just the pairs: `comment_bodies` is keyed by
    // position in `comments`, so this is what lets the flat thread reuse the
    // bodies already rendered above instead of rendering them a second time.
    let thread_indexes: Vec<usize> = comments
        .iter()
        .enumerate()
        .filter(|(index, _)| !placed.is_some_and(|placed| placed.contains(index)))
        .map(|(index, _)| index)
        .collect();
    let thread: Vec<_> = thread_indexes.iter().map(|&index| &comments[index]).collect();

    // A cross-fork MR's source branch lives on a different repo — resolve
    // that repo's current display name (best-effort: a deleted fork just
    // falls back to its raw uid, same dangling-fork tolerance
    // `webui::repo::summary::render_summary`'s `forked_from` link already has)
    // so the source shows as `owner/name:branch`, not a bare branch name
    // that'd otherwise look same-repo.
    let source_label = if mr.source_repo == target.uid {
        mr.source_branch.clone()
    } else {
        let (source_config, etag) = ctx.remote_for_uid(&mr.source_repo).get_repo_config().await?;
        let repo_label = if etag.is_some() {
            source_config.full_name
        } else {
            mr.source_repo.to_string()
        };
        format!("{repo_label}:{}", mr.source_branch)
    };
    let description_html = render_body(ctx, target, repo_name, &mr.description).await;
    let thread_htmls: Vec<String> = thread_indexes
        .iter()
        .map(|&index| comment_bodies[index].clone())
        .collect();
    let mut review_htmls = Vec::with_capacity(reviews.len());
    for review in &reviews {
        review_htmls.push(render_body(ctx, target, repo_name, &review.body).await);
    }

    Ok(html! {
        h3 { "#" (mr.number) " " (mr.title) }
        p class="muted" {
            code { (source_label) } " -> " code { (mr.target_branch) }
        }
        p class="muted" { (t::opened_by_prefix(locale)) (mr.author) (t::opened_by_middle(locale)) (mr.created_at) }
        @if !mr.labels.is_empty() {
            p {
                @for name in &mr.labels {
                    (super::components::label_pill(name, label_colors.get(name.as_str()).copied().unwrap_or("#999999")))
                    " "
                }
            }
        }
        @if !mr.assignees.is_empty() {
            p class="muted" { (t::assignees_label(locale)) ": " (mr.assignees.join(", ")) }
        }
        @if !mr.requested_reviewers.is_empty() {
            p class="muted" { (t::reviewers_label(locale)) ": " (mr.requested_reviewers.join(", ")) }
        }
        @if !mr.description.is_empty() {
            div class="readme plain" { (PreEscaped(&description_html)) }
        }
        @if let Some(paths) = &conflicts {
            (render_conflict_notice(repo_name, &mr, paths, locale))
        }
        (render_mr_actions(
            &mr,
            can_manage,
            can_merge,
            mr.source_repo == target.uid,
            fast_forwardable,
            repo_name,
            &csrf_field(headers),
            locale,
        ))

        @if can_manage {
            h4 { (t::edit_metadata_heading(locale)) }
            form method="post" action="/_merges/edit_metadata" {
                (PreEscaped(csrf_field(headers)))
                input type="hidden" name="repo" value=(repo_name);
                input type="hidden" name="number" value=(mr.number);
                div class="field" {
                    label { (t::title_label(locale)) }
                    input name="title" size="40" value=(mr.title);
                }
                div class="field" {
                    label { (t::description_label(locale)) }
                    textarea name="description" rows="4" cols="60" { (mr.description) }
                }
                div class="field" {
                    label { (t::labels_label(locale)) }
                    input name="labels" size="40" value=(mr.labels.join(", ")) placeholder=(t::labels_placeholder(locale));
                }
                div class="field" {
                    label { (t::assignees_label(locale)) }
                    input name="assignees" size="40" value=(mr.assignees.join(", ")) placeholder=(t::assignees_placeholder(locale));
                }
                div class="field" {
                    label { (t::reviewers_label(locale)) }
                    input name="reviewers" size="40" value=(mr.requested_reviewers.join(", ")) placeholder=(t::reviewers_placeholder(locale));
                }
                p { button type="submit" { (t::save_button(locale)) } }
            }
        }

        @if d.commits.is_empty() {
            p class="muted" { (t::no_commits_hint(locale)) }
        } @else {
            (commit_table(repo_name, &d.commits, &commit_avatars, locale))
        }
        @if !d.stat.trim().is_empty() {
            div class="scroll-x" { pre class="diff" { (d.stat) } }
        }
        p { (PreEscaped(render_diff_view_buttons(&return_to, diff_view))) }
        @match &annotated {
            Some(annotated) => div class="scroll-x" { (annotated.markup) },
            None => div class="scroll-x full-bleed" { (side_by_side_diff(&d.diff)) },
        }

        (render_reviews(&reviews, &review_htmls, repo_name, mr.number, viewer.as_deref(), &mr.author, headers, locale))

        h4 { (t::comments_heading(locale)) }
        @if thread.is_empty() {
            p class="muted" { (t::no_comments_hint(locale)) }
        }
        @for (i, (comment_id, comment)) in thread.iter().enumerate() {
            div class="commit-meta" {
                p class="muted" {
                    strong { (comment.author) } " — " (comment.created_at)
                    @if let Some(anchor) = &comment.anchor {
                        " — " (t::line_comment_prefix(locale)) code { (anchor.path) ":" (anchor.line) }
                    }
                }
                div class="readme plain" { (PreEscaped(&thread_htmls[i])) }
                @let is_comment_author = viewer.as_deref() == Some(comment.author.as_str());
                (super::components::comment_edit_delete_controls(
                    "/_merges",
                    repo_name,
                    mr.number,
                    comment_id,
                    &comment.body,
                    is_comment_author,
                    is_comment_author || can_merge,
                    headers,
                    locale,
                ))
            }
        }
        @if logged_in {
            form method="post" action="/_merges/comment" {
                (PreEscaped(csrf_field(headers)))
                input type="hidden" name="repo" value=(repo_name);
                input type="hidden" name="number" value=(mr.number);
                textarea name="body" rows="4" cols="60" {}
                p { button type="submit" { (t::comment_button(locale)) } }
            }
        }
    }
    .into_string())
}

#[cfg(any(test, feature = "test-util"))]
pub mod annotated_diff;
#[cfg(not(any(test, feature = "test-util")))]
mod annotated_diff;
#[cfg(any(test, feature = "test-util"))]
pub mod handlers;
#[cfg(not(any(test, feature = "test-util")))]
mod handlers;

pub(crate) use handlers::{
    handler_bulk_post, handler_close_post, handler_comment_delete_post, handler_comment_edit_post,
    handler_comment_post, handler_create_post, handler_edit_metadata_post, handler_mark_draft_post,
    handler_mark_ready_post, handler_merge_post, handler_reopen_post, handler_review_post,
};
