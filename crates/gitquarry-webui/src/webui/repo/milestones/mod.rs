//! The `milestones` view: a repo's open/closed milestones, plus an inline
//! create form — no separate `new_milestone` page the way issues/pulls
//! have one, since a milestone is just a title/description/due-date, not
//! enough fields to earn its own page. The `/_milestones*` mutation
//! handlers this view's forms POST to live in [`handlers`], same
//! top-level-route shape every other repo mutation set uses.

use crate::auth::web::login;
use crate::webui::{AppError, csrf_field, tracker_client};
use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::Permission;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::milestones as t;
use gitquarry_wire::tracker::{MilestoneStatus, MilestoneSummaryDto};
use maud::{Markup, PreEscaped, html};

fn tracker_unavailable(locale: Locale) -> AppError {
    AppError::service_unavailable(anyhow::anyhow!(
        "{}",
        gitquarry_i18n::shared::tracker::tracker_not_configured(locale)
    ))
}

/// `true` if the current session may create/close/reopen milestones:
/// `Developer`+ on the repo, no author exemption — a milestone isn't
/// "owned" by whoever created it, it's repo management like a label, same
/// bar `pulls::handlers::require_developer_permission` gates merging with.
async fn can_manage(ctx: &AppContext, headers: &HeaderMap, repo_name: &str) -> Result<bool, AppError> {
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

fn render_milestone_row(
    milestone: &MilestoneSummaryDto,
    can_manage: bool,
    repo_name: &str,
    csrf_html: &str,
    locale: Locale,
) -> Markup {
    html! {
        div class="commit-meta" {
            p { strong { "#" (milestone.number) " " (milestone.title) } }
            p class="muted" { (t::progress_summary(locale, milestone.open_issues, milestone.closed_issues)) }
            @if !milestone.due_date.is_empty() {
                p class="muted" { (t::due_date_prefix(locale)) (milestone.due_date) }
            }
            @if !milestone.description.is_empty() {
                p { (milestone.description) }
            }
            @if can_manage {
                @match milestone.status {
                    MilestoneStatus::Open => {
                        form method="post" action="/_milestones/close" class="inline-form" {
                            (PreEscaped(csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="number" value=(milestone.number);
                            button type="submit" { (t::close_button(locale)) }
                        }
                    }
                    MilestoneStatus::Closed => {
                        form method="post" action="/_milestones/reopen" class="inline-form" {
                            (PreEscaped(csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="number" value=(milestone.number);
                            button type="submit" { (t::reopen_button(locale)) }
                        }
                    }
                }
                details {
                    summary { (t::edit_button(locale)) }
                    form method="post" action="/_milestones/edit" {
                        (PreEscaped(csrf_html))
                        input type="hidden" name="repo" value=(repo_name);
                        input type="hidden" name="number" value=(milestone.number);
                        div class="field" {
                            label { (t::title_label(locale)) }
                            input name="title" size="40" value=(milestone.title);
                        }
                        div class="field" {
                            label { (t::description_label(locale)) }
                            textarea name="description" rows="4" cols="60" { (milestone.description) }
                        }
                        div class="field" {
                            label { (t::due_date_label(locale)) }
                            input name="due_date" size="20" value=(milestone.due_date);
                        }
                        p { button type="submit" { (t::save_button(locale)) } }
                    }
                }
            }
        }
    }
}

/// A compact checkbox-per-milestone strip, separate from
/// [`render_milestone_row`]'s own per-row close/reopen/edit forms below it
/// (siblings, not nested — a `<form>` can't contain another `<form>`, so
/// this can't just add checkboxes inside the existing per-row markup) — the
/// only way to offer "close/reopen several at once" without touching the
/// already-working single-item flow each card still has on its own.
fn render_bulk_milestones_form(
    open: &[MilestoneSummaryDto],
    closed: &[MilestoneSummaryDto],
    repo_name: &str,
    csrf_html: &str,
    locale: Locale,
) -> Markup {
    html! {
        details {
            summary { (t::bulk_select_heading(locale)) }
            form method="post" action="/_milestones/bulk" {
                (PreEscaped(csrf_html))
                input type="hidden" name="repo" value=(repo_name);
                @for milestone in open.iter().chain(closed.iter()) {
                    label class="field" {
                        input type="checkbox" name="numbers" value=(milestone.number);
                        " #" (milestone.number) " " (milestone.title)
                    }
                }
                p {
                    select name="action" {
                        option value="close" { (t::bulk_close_option(locale)) }
                        option value="reopen" { (t::bulk_reopen_option(locale)) }
                    }
                    button type="submit" { (t::bulk_apply_button(locale)) }
                }
            }
        }
    }
}

pub async fn render_milestones_list(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };
    let milestones = tracker_client::list_milestones(tracker, &target.uid).await?;
    let (open, closed): (Vec<_>, Vec<_>) = milestones.into_iter().partition(|m| m.status == MilestoneStatus::Open);
    let can_manage = can_manage(ctx, headers, repo_name).await?;
    let csrf_html = csrf_field(headers);

    Ok(html! {
        h3 { (t::milestones_heading(locale)) }
        @if can_manage && open.len() + closed.len() > 1 {
            (render_bulk_milestones_form(&open, &closed, repo_name, &csrf_html, locale))
        }
        h4 { (t::open_milestones_heading(locale)) }
        @if open.is_empty() {
            p class="muted" { (t::no_open_milestones_hint(locale)) }
        }
        @for milestone in &open {
            (render_milestone_row(milestone, can_manage, repo_name, &csrf_html, locale))
        }
        h4 { (t::closed_milestones_heading(locale)) }
        @if closed.is_empty() {
            p class="muted" { (t::no_closed_milestones_hint(locale)) }
        }
        @for milestone in &closed {
            (render_milestone_row(milestone, can_manage, repo_name, &csrf_html, locale))
        }
        @if can_manage {
            h4 { (t::new_milestone_heading(locale)) }
            form method="post" action="/_milestones" {
                (PreEscaped(&csrf_html))
                input type="hidden" name="repo" value=(repo_name);
                div class="field" {
                    label { (t::title_label(locale)) }
                    input name="title" size="40";
                }
                div class="field" {
                    label { (t::description_label(locale)) }
                    textarea name="description" rows="4" cols="60" {}
                }
                div class="field" {
                    label { (t::due_date_label(locale)) }
                    input name="due_date" size="20";
                }
                p { button type="submit" { (t::create_milestone_button(locale)) } }
            }
        }
    }
    .into_string())
}

/// A `<select>` of currently-open milestones plus a "none" option — used
/// by `webui::repo::issues::render_issue_detail`'s edit form. Closed
/// milestones aren't offered here (assigning to an already-closed
/// milestone is unusual enough to not be worth the extra `<option>`s), but
/// an issue that already points at one still shows its title in the
/// detail page's own read-only summary line regardless of status — that
/// lookup happens inline in `render_issue_detail`, not through a helper
/// here.
pub fn render_milestone_select(
    open_milestones: &[MilestoneSummaryDto],
    selected: Option<u64>,
    locale: Locale,
) -> Markup {
    html! {
        select name="milestone" {
            option value="" selected[selected.is_none()] { (t::no_milestone_option(locale)) }
            @for milestone in open_milestones {
                option value=(milestone.number) selected[selected == Some(milestone.number)] {
                    "#" (milestone.number) " " (milestone.title)
                }
            }
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub mod handlers;
#[cfg(not(any(test, feature = "test-util")))]
mod handlers;

pub(crate) use handlers::{
    handler_bulk_post, handler_close_post, handler_create_post, handler_edit_post, handler_reopen_post,
};
