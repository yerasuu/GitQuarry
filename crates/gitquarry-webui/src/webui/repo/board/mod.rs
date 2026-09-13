//! The `board` view: one kanban board per repo, columns of cards pointing
//! at an issue, an MR, or a freeform note. Reordering within a column is
//! still plain forms (swap up/down — the backend has no "move to this
//! exact position" call to drive anything richer). Moving a card to a
//! *different* column can now also be done by dragging it there
//! (`app.js`'s `dragstart`/`dragover`/`drop` listeners, keyed off
//! `draggable`/`data-column-index` below) — an optimistic DOM move plus a
//! fire-and-forget POST to the same `/_board/move_to_column` form already
//! rendered per card, no new route. The manual "move to column" `<select>`
//! form stays as the no-JS fallback either way. The `/_board*` mutation
//! handlers these forms POST to live in [`handlers`], same top-level-route
//! shape `webui::repo::issues`/`milestones` use.
//!
//! Every read here except the `CardRef::MergeRequest` case goes through
//! `ctx.tracker` (`webui::tracker_client`) — `gitquarry-tracker` is its own
//! binary now (`docs/tracker-binary-split-concept.md`). `MergeRequest` cards
//! still resolve via core's own in-process `domain::merge_request`, since
//! MRs stay a kernel concept, not a tracker one.

use crate::auth::web::login;
use crate::webui::{AppError, csrf_field, repo_link, tracker_client};
use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::domain;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::board as t;
use gitquarry_wire::MrStatus;
use gitquarry_wire::tracker::{BoardCardDto, CardRefDto, IssueStatus};
use maud::{Markup, PreEscaped, html};

fn tracker_unavailable(locale: Locale) -> AppError {
    AppError::service_unavailable(anyhow::anyhow!(
        "{}",
        gitquarry_i18n::shared::tracker::tracker_not_configured(locale)
    ))
}

/// `Developer`+ on the repo, no author exemption — a board is repo
/// management like a label/milestone, not something an individual card's
/// creator "owns" (there's no meaningful author to track for a card at
/// all: `Note` cards are freeform, `Issue`/`MergeRequest` cards already have
/// their own author elsewhere).
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

async fn render_card_body(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    card: &CardRefDto,
    locale: Locale,
) -> Markup {
    match card {
        CardRefDto::Issue(number) => {
            let issue = match &ctx.tracker {
                Some(tracker) => tracker_client::get_issue(tracker, &target.uid, *number)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            match issue {
                Some(issue) => html! {
                    a href=(repo_link(repo_name, "issue", "").number(*number).to_string()) { "#" (number) " " (issue.title) }
                    @if issue.status == IssueStatus::Closed {
                        " " span class="badge" { (t::closed_badge(locale)) }
                    }
                },
                None => html! { em { (t::broken_reference_hint(locale)) } },
            }
        }
        CardRefDto::MergeRequest(number) => match domain::merge_request::get(ctx, target, *number).await {
            Ok(Some(mr)) => html! {
                a href=(repo_link(repo_name, "merge", "").number(*number).to_string()) { "#" (number) " " (mr.title) }
                @if mr.status != MrStatus::Open {
                    " " span class="badge" {
                        (if mr.status == MrStatus::Merged { t::merged_badge(locale) } else { t::closed_badge(locale) })
                    }
                }
            },
            _ => html! { em { (t::broken_reference_hint(locale)) } },
        },
        CardRefDto::Note(text) => html! { (text) },
    }
}

/// Bundles [`render_card`]'s less-central params — same reasoning as
/// [`crate::webui::page_chrome::HeaderExtras`] (adding `locale` as an 8th
/// positional arg would trip clippy's `too_many_arguments`).
#[derive(Clone, Copy)]
struct CardRenderCtx<'a> {
    repo_name: &'a str,
    column_count: usize,
    column_index: usize,
    can_manage: bool,
    csrf_html: &'a str,
    locale: Locale,
}

async fn render_card(
    ctx: &AppContext,
    target: &ResolvedRepo,
    card: &BoardCardDto,
    render: &CardRenderCtx<'_>,
) -> Markup {
    let CardRenderCtx {
        repo_name,
        column_count,
        column_index,
        can_manage,
        csrf_html,
        locale,
    } = *render;
    let body = render_card_body(ctx, target, repo_name, &card.card, locale).await;
    html! {
        div class="commit-meta" draggable=[can_manage.then_some("true")] {
            p { (body) }
            @if can_manage {
                form method="post" action="/_board/move_up" class="inline-form" {
                    (PreEscaped(csrf_html))
                    input type="hidden" name="repo" value=(repo_name);
                    input type="hidden" name="card_id" value=(card.id);
                    button type="submit" { "↑" }
                }
                form method="post" action="/_board/move_down" class="inline-form" {
                    (PreEscaped(csrf_html))
                    input type="hidden" name="repo" value=(repo_name);
                    input type="hidden" name="card_id" value=(card.id);
                    button type="submit" { "↓" }
                }
                @if column_count > 1 {
                    form method="post" action="/_board/move_to_column" class="inline-form" {
                        (PreEscaped(csrf_html))
                        input type="hidden" name="repo" value=(repo_name);
                        input type="hidden" name="card_id" value=(card.id);
                        select name="to_column" {
                            @for i in 0..column_count {
                                @if i != column_index {
                                    option value=(i) { (t::move_to_column_option(locale, i + 1)) }
                                }
                            }
                        }
                        button type="submit" { (t::move_button(locale)) }
                    }
                }
                form method="post" action="/_board/remove_card" class="inline-form" {
                    (PreEscaped(csrf_html))
                    input type="hidden" name="repo" value=(repo_name);
                    input type="hidden" name="card_id" value=(card.id);
                    button type="submit" { (t::remove_card_button(locale)) }
                }
            }
        }
    }
}

pub async fn render_board(
    ctx: &AppContext,
    target: &ResolvedRepo,
    repo_name: &str,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let Some(tracker) = &ctx.tracker else {
        return Err(tracker_unavailable(locale));
    };
    let board = tracker_client::get_board(tracker, &target.uid).await?;
    let can_manage = can_manage(ctx, headers, repo_name).await?;
    let csrf_html = csrf_field(headers);
    let column_count = board.columns.len();

    let mut columns_markup = Vec::with_capacity(column_count);
    for (column_index, column) in board.columns.iter().enumerate() {
        let mut cards_markup = Vec::with_capacity(column.cards.len());
        let render_ctx = CardRenderCtx {
            repo_name,
            column_count,
            column_index,
            can_manage,
            csrf_html: &csrf_html,
            locale,
        };
        for card in &column.cards {
            cards_markup.push(render_card(ctx, target, card, &render_ctx).await);
        }
        columns_markup.push(html! {
            div class="board-column" data-column-index=(column_index) {
                h4 { (column.name) }
                @for card in &cards_markup {
                    (card)
                }
                @if can_manage {
                    @if column.cards.is_empty() {
                        form method="post" action="/_board/remove_column" class="inline-form" {
                            (PreEscaped(&csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="column" value=(column_index);
                            button type="submit" { (t::remove_column_button(locale)) }
                        }
                    }
                    details {
                        summary { (t::add_card_summary(locale)) }
                        form method="post" action="/_board/add_card" {
                            (PreEscaped(&csrf_html))
                            input type="hidden" name="repo" value=(repo_name);
                            input type="hidden" name="column" value=(column_index);
                            div class="field" {
                                label { (t::card_kind_label(locale)) }
                                select name="kind" {
                                    option value="note" { (t::card_kind_note(locale)) }
                                    option value="issue" { (t::card_kind_issue(locale)) }
                                    option value="merge_request" { (t::card_kind_merge_request(locale)) }
                                }
                            }
                            div class="field" {
                                label { (t::card_value_label(locale)) }
                                input name="value" size="40" placeholder=(t::card_value_placeholder(locale));
                            }
                            p { button type="submit" { (t::add_card_button(locale)) } }
                        }
                    }
                }
            }
        });
    }

    Ok(html! {
        h3 { (t::board_heading(locale)) }
        @if board.columns.is_empty() {
            div class="empty-state" { div class="empty-state-title" { (t::no_columns_hint(locale)) } }
        }
        div class="board-columns" {
            @for column in &columns_markup {
                (column)
            }
        }
        @if can_manage {
            h4 { (t::new_column_heading(locale)) }
            form method="post" action="/_board/add_column" {
                (PreEscaped(&csrf_html))
                input type="hidden" name="repo" value=(repo_name);
                input name="name" size="30" placeholder=(t::new_column_placeholder(locale));
                button type="submit" { (t::add_column_button(locale)) }
            }
        }
    }
    .into_string())
}

#[cfg(any(test, feature = "test-util"))]
pub mod handlers;
#[cfg(not(any(test, feature = "test-util")))]
mod handlers;

pub(crate) use handlers::{
    handler_add_card_post, handler_add_column_post, handler_move_card_down_post, handler_move_card_to_column_post,
    handler_move_card_up_post, handler_remove_card_post, handler_remove_column_post,
};
