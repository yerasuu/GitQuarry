//! The `issues`/`issue`/`new_issue` views. The `/_issues*` mutation
//! handlers those views' forms POST to live in [`handlers`]: top-level
//! routes, not nested under the repo's own `/{*repo}` path — same shape as
//! `/_pulls` (`super::pulls`'s doc comment explains why).
//!
//! Unlike a pull request, an issue has no merge/diff/shadow-ref machinery —
//! just a title, a markdown-free body, a status, labels, and assignees.
//!
//! Every read here goes through `ctx.tracker` (`webui::tracker_client`) —
//! `gitquarry-tracker` is its own binary now, never a Rust dependency of
//! this crate (`docs/tracker-binary-split-concept.md`).

use crate::auth::web::login;
use crate::webui::{AppError, csrf_field, repo_link, tracker_client, url_encode};
use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::Permission;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::issues as t;
use gitquarry_wire::tracker::{IssueCommentDto, IssueStatus, IssueSummary, MilestoneStatus};
use maud::{Markup, PreEscaped, html};

fn tracker_unavailable(locale: Locale) -> AppError {
    AppError::service_unavailable(anyhow::anyhow!(
        "{}",
        gitquarry_i18n::shared::tracker::tracker_not_configured(locale)
    ))
}

fn status_label(status: IssueStatus, locale: Locale) -> String {
    match status {
        IssueStatus::Open => t::open_status(locale),
        IssueStatus::Closed => t::closed_status(locale),
    }
}

/// Same `styler` badge variants the pulls-list status pill already uses —
/// open is `.badge.ok`, closed stays the neutral bare `.badge` (no merged
/// state for an issue).
fn status_badge_class(status: IssueStatus) -> &'static str {
    match status {
        IssueStatus::Open => "badge ok",
        IssueStatus::Closed => "badge",
    }
}

/// Status line + close/reopen button, wrapped in `#issue-actions` — same
/// `data-async`/`data-async-target` fragment shape
/// `webui::repo::pulls::render_pr_actions` already uses, so closing/
/// reopening an issue refreshes just this block instead of the whole page.
fn render_issue_actions(
    issue: &IssueSummary,
    can_manage: bool,
    repo_name: &str,
    csrf_html: &str,
    locale: Locale,
) -> Markup {
    html! {
        div id="issue-actions" {
            p class="muted" { (status_label(issue.status, locale)) }
            @if can_manage {
                @match issue.status {
                    IssueStatus::Open => {
                        form method="post" action="/_issues/close" class="inline-form" data-async data-async-target="#issue-actions" {
                            (PreEscaped(csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="number" value=(issue.number);
                            button type="submit" { (t::close_button(locale)) }
                        }
                    }
                    IssueStatus::Closed => {
                        form method="post" action="/_issues/reopen" class="inline-form" data-async data-async-target="#issue-actions" {
                            (PreEscaped(csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="number" value=(issue.number);
                            button type="submit" { (t::reopen_button(locale)) }
                        }
                    }
                }
            }
        }
    }
}

/// `status` is one of `"open"`/`"closed"`/`"all"`; an unrecognized value
/// behaves like `"all"` (see [`render_issues_list`]'s own filter).
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
            format!("/{}?view=issues&status={id}{q_suffix}", url_encode(repo_name)),
            label,
        )
    })
    .collect();
    html! {
        // `.subnav`: stacks directly under `page_chrome::project_subnav`'s
        // own row -- see that modifier's CSS doc comment. No hamburger
        // toggle (`tab_links`'s own `toggle: None`) -- only 3 entries,
        // already reads fine as a short always-visible stack on mobile.
        table class="tabs subnav" {
            tr {
                td { (crate::webui::tab_links(None, &tabs, status)) }
            }
        }
    }
}

/// `status` narrows to `"open"`/`"closed"`, anything else (including
/// `"all"`) shows every issue. `q`, when given, further keeps only issues
/// whose title or body contains it (case-insensitive) — same
/// substring-`retain()` shape `auth::web::users::handler_users_get`/
/// `groups::handler_groups_get` already use for their own `?q=` filters,
/// applied at this layer rather than tracker's own list query for the same
/// reason: a repo's issue count is small enough that filtering the
/// already-fetched `Vec` in place is simpler than adding a text-search
/// parameter to the loopback call.
pub async fn render_issues_list(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    status: &str,
    q: Option<&str>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };
    let mut issues = tracker_client::list_issues(tracker, &target.uid, None).await?;
    match status {
        "open" => issues.retain(|issue| issue.status == IssueStatus::Open),
        "closed" => issues.retain(|issue| issue.status == IssueStatus::Closed),
        _ => {}
    }
    if let Some(q) = q {
        let q_lower = q.to_ascii_lowercase();
        issues.retain(|issue| {
            issue.title.to_ascii_lowercase().contains(&q_lower) || issue.body.to_ascii_lowercase().contains(&q_lower)
        });
    }
    let avatars = gitquarry_core::auth::access::avatar_content_types(
        &ctx.backend(),
        &ctx.cipher,
        issues.iter().map(|issue| issue.author.clone()),
    )
    .await;
    let logged_in = login::current_user(ctx, headers).await.is_some();
    let can_bulk_manage = can_moderate_comments(ctx, headers, repo_name).await?;
    let narrowed = q.is_some() || status != "open";
    let no_matches_hint = if narrowed {
        t::no_matches_hint(locale)
    } else {
        t::no_issues_yet_hint(locale)
    };
    let csrf_html = csrf_field(headers);

    Ok(html! {
        (status_tabs(repo_name, status, q, locale))
        form method="get" action=(format!("/{}", url_encode(repo_name))) {
            input type="hidden" name="view" value="issues";
            input type="hidden" name="status" value=(status);
            input name="q" value=(q.unwrap_or("")) placeholder=(t::search_issues_placeholder(locale));
            button type="submit" { (t::search_button(locale)) }
        }
        @if can_bulk_manage {
            form method="post" action="/_issues/bulk" {
                (PreEscaped(&csrf_html))
                input type="hidden" name="repo" value=(repo_name);
                div class="table-scroll" {
                table class="list" {
                    tr {
                        th {}
                        th { (t::number_column(locale)) } th { (t::title_column(locale)) } th { (t::status_column(locale)) } th { (t::author_column(locale)) }
                    }
                    @if issues.is_empty() {
                        tr { td colspan="5" { div class="empty-state" { div class="empty-state-title" { (no_matches_hint) } } } }
                    }
                    @for issue in &issues {
                        tr {
                            td { input type="checkbox" name="numbers" value=(issue.number); }
                            td { (issue.number) }
                            td { a href=(repo_link(repo_name, "issue", "").number(issue.number).to_string()) { (issue.title) } }
                            td { span class=(status_badge_class(issue.status)) { (status_label(issue.status, locale)) } }
                            td { (crate::webui::identicon::account_avatar(&issue.author, avatars.get(&issue.author).map(String::as_str))) " " (issue.author) }
                        }
                    }
                }
                }
                @if !issues.is_empty() {
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
                @if issues.is_empty() {
                    tr { td colspan="4" { div class="empty-state" { div class="empty-state-title" { (no_matches_hint) } } } }
                }
                @for issue in &issues {
                    tr {
                        td { (issue.number) }
                        td { a href=(repo_link(repo_name, "issue", "").number(issue.number).to_string()) { (issue.title) } }
                        td { span class=(status_badge_class(issue.status)) { (status_label(issue.status, locale)) } }
                        td { (crate::webui::identicon::account_avatar(&issue.author, avatars.get(&issue.author).map(String::as_str))) " " (issue.author) }
                    }
                }
            }
            }
        }
        // `class="btn"` on a plain `<a>`, not `<button>`: this is a GET
        // navigation (a new page, not a mutation), so a real
        // `<button type="submit">` would need a whole `<form>` wrapper just
        // to look right -- styler's own `button, .btn {...}` selector
        // styles either element identically. No `milestones` cross-link
        // here anymore -- `page_chrome::project_subnav` (this tab's own
        // sub-nav, rendered above) already covers that.
        @if logged_in {
            p { a class="btn" href=(format!("/{}?view=new_issue", url_encode(repo_name))) { (t::new_issue_button(locale)) } }
        }
    }
    .into_string())
}

/// A GET form (linkable/bookmarkable, unlike the settings tab's POST
/// forms) — same shape as `pulls::render_new_pull_form`, minus the
/// branch-picking fields an issue has no use for.
/// `template` is `ISSUE_TEMPLATE.md`'s content from the repo's default
/// branch, if the file exists — pre-fills the body textarea the same way
/// GitHub's own issue template does, editable/clearable before submit like
/// any other pre-filled form field, never enforced at the handler.
/// `title`/`body` are the submitted values to redisplay after a rejected
/// `POST /_issues` (empty title, ...) — `""` for the plain GET landing,
/// same shape `template` already prefilled `body` with before the form
/// could fail. `error`, if `Some`, is that rejection's message.
pub fn render_new_issue_form(
    repo_name: &str,
    template: Option<&str>,
    title: &str,
    body: &str,
    error: Option<&str>,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    html! {
        h3 { (t::new_issue_heading(locale)) }
        @if let Some(msg) = error {
            p class="alert alert-err" { (msg) }
        }
        form method="post" action="/_issues" {
            (PreEscaped(csrf_field(headers)))
            input type="hidden" name="repo" value=(repo_name);
            div class="field" {
                label { (t::title_label(locale)) }
                input name="title" size="40" value=(title);
            }
            div class="field" {
                label { (t::body_label(locale)) }
                textarea name="body" rows="4" cols="60" { (if body.is_empty() { template.unwrap_or("") } else { body }) }
            }
            p { button type="submit" { (t::create_issue_button(locale)) } }
        }
    }
}

/// `true` if the current session can edit/close/reopen this issue: its own
/// author, or `Developer`+ on the repo — same bar and same "the mutation
/// handlers re-check this themselves" reasoning as
/// `pulls::can_manage`.
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

/// `true` if the current session is `Developer`+ on the repo — no author
/// exemption (unlike [`can_manage`]), since this only ever grants the
/// *delete* override on someone else's comment (`is_moderator`), never edit.
async fn can_moderate_comments(ctx: &AppContext, headers: &HeaderMap, repo_name: &str) -> Result<bool, AppError> {
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

/// An issue's own body, or one of its thread comments, as HTML: resolve the
/// `#123` mentions, then render — same two-step treatment `merges::render_body`
/// gives an MR's own body/comments/reviews, for the same untrusted-markdown
/// reason.
async fn render_body(ctx: &AppContext, target: &ResolvedRepo, repo_name: &str, body: &str) -> String {
    let mentions = super::components::resolve_number_mentions(ctx, target, repo_name, body).await;
    crate::webui::markdown::render_with_mentions(body, &mentions)
}

pub async fn render_issue_detail(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    number: u64,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };
    let Some(issue) = tracker_client::get_issue(tracker, &target.uid, number).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::tracker::issue_not_found(locale, number)
        )));
    };
    let can_manage = can_manage(ctx, headers, repo_name, &issue.author).await?;
    let can_moderate_comments = can_moderate_comments(ctx, headers, repo_name).await?;
    let comments: Vec<IssueCommentDto> = tracker_client::list_issue_comments(tracker, &target.uid, number).await?;
    let viewer = login::current_user(ctx, headers).await.map(|u| u.username);
    let logged_in = viewer.is_some();
    let (config, _etag) = target.remote.get_repo_config().await?;
    let label_colors: std::collections::HashMap<&str, &str> = config
        .labels
        .iter()
        .map(|l| (l.name.as_str(), l.color.as_str()))
        .collect();
    let milestones = tracker_client::list_milestones(tracker, &target.uid).await?;
    let milestone_title = issue
        .milestone
        .and_then(|number| milestones.iter().find(|m| m.number == number))
        .map(|m| format!("#{} {}", m.number, m.title));
    let open_milestones: Vec<_> = milestones
        .into_iter()
        .filter(|m| m.status == MilestoneStatus::Open)
        .collect();
    let csrf_html = csrf_field(headers);
    let body_html = render_body(ctx, target, repo_name, &issue.body).await;
    let mut comment_htmls = Vec::with_capacity(comments.len());
    for comment in &comments {
        comment_htmls.push(render_body(ctx, target, repo_name, &comment.body).await);
    }

    Ok(html! {
        h3 { "#" (issue.number) " " (issue.title) }
        p class="muted" { (t::opened_by_prefix(locale)) (issue.author) (t::opened_by_middle(locale)) (issue.created_at) }
        @if !issue.labels.is_empty() {
            p {
                @for name in &issue.labels {
                    (super::components::label_pill(name, label_colors.get(name.as_str()).copied().unwrap_or("#999999")))
                    " "
                }
            }
        }
        @if !issue.assignees.is_empty() {
            p class="muted" { (t::assignees_label(locale)) ": " (issue.assignees.join(", ")) }
        }
        @if let Some(title) = &milestone_title {
            p class="muted" { (t::milestone_label(locale)) ": " (title) }
        }
        @if !issue.body.is_empty() {
            p { (PreEscaped(&body_html)) }
        }
        (render_issue_actions(&issue, can_manage, repo_name, &csrf_html, locale))

        @if can_manage {
            h4 { (t::edit_heading(locale)) }
            form method="post" action="/_issues/edit" {
                (PreEscaped(&csrf_html))
                input type="hidden" name="repo" value=(repo_name);
                input type="hidden" name="number" value=(issue.number);
                div class="field" {
                    label { (t::title_label(locale)) }
                    input name="title" size="40" value=(issue.title);
                }
                div class="field" {
                    label { (t::body_label(locale)) }
                    textarea name="body" rows="4" cols="60" { (issue.body) }
                }
                div class="field" {
                    label { (t::labels_label(locale)) }
                    input name="labels" size="40" value=(issue.labels.join(", ")) placeholder=(t::labels_placeholder(locale));
                }
                div class="field" {
                    label { (t::assignees_label(locale)) }
                    input name="assignees" size="40" value=(issue.assignees.join(", ")) placeholder=(t::assignees_placeholder(locale));
                }
                div class="field" {
                    label { (t::milestone_label(locale)) }
                    (super::milestones::render_milestone_select(&open_milestones, issue.milestone, locale))
                }
                p { button type="submit" { (t::save_button(locale)) } }
            }
        }

        h4 { (t::comments_heading(locale)) }
        @if comments.is_empty() {
            p class="muted" { (t::no_comments_hint(locale)) }
        }
        @for (i, comment) in comments.iter().enumerate() {
            div class="commit-meta" {
                p class="muted" { strong { (comment.author) } " — " (comment.created_at) }
                p { (PreEscaped(&comment_htmls[i])) }
                @let is_comment_author = viewer.as_deref() == Some(comment.author.as_str());
                @if is_comment_author {
                    details {
                        summary { (t::edit_comment_button(locale)) }
                        form method="post" action="/_issues/comment/edit" {
                            (PreEscaped(&csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="number" value=(issue.number);
                            input type="hidden" name="comment_id" value=(comment.id);
                            textarea name="body" rows="4" cols="60" { (comment.body) }
                            p { button type="submit" { (t::save_button(locale)) } }
                        }
                    }
                }
                @if is_comment_author || can_moderate_comments {
                    form method="post" action="/_issues/comment/delete" class="inline-form" data-confirm=(t::delete_comment_confirm(locale)) {
                        (PreEscaped(&csrf_html))
                        input type="hidden" name="repo" value=(repo_name);
                        input type="hidden" name="number" value=(issue.number);
                        input type="hidden" name="comment_id" value=(comment.id);
                        button type="submit" { (t::delete_comment_button(locale)) }
                    }
                }
            }
        }
        @if logged_in {
            form method="post" action="/_issues/comment" {
                (PreEscaped(&csrf_html))
                input type="hidden" name="repo" value=(repo_name);
                input type="hidden" name="number" value=(issue.number);
                textarea name="body" rows="4" cols="60" {}
                p { button type="submit" { (t::comment_button(locale)) } }
            }
        }
    }
    .into_string())
}

mod handlers;

pub(crate) use handlers::{
    handler_bulk_post, handler_close_post, handler_comment_delete_post, handler_comment_edit_post,
    handler_comment_post, handler_create_post, handler_edit_post, handler_reopen_post,
};
