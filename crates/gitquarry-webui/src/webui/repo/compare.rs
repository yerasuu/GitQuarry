//! The `compare` view: full diff between two arbitrary refs (branches or
//! tags), not just one commit's — see [`gitquarry_core::git::browse::diff_range`].
//! Reached from a small form on the `refs` tab, not a top-level nav tab:
//! there's no single "current" comparison the way there's a current branch.

use super::components::{color_diff, commit_avatars, commit_table, side_by_side_diff};
use crate::webui::{DiffView, effective_diff_view, render_diff_view_buttons, url_encode};
use anyhow::Result;
use axum::http::HeaderMap;
use gitquarry_core::app::AppContext;
use gitquarry_core::git;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo_compare as t;
use maud::{Markup, PreEscaped, html};
use std::path::Path as FsPath;

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_compare(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    base: &str,
    head: &str,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String> {
    let stat = git::browse::diffstat_range(bare_repo, base, head).await?;
    let diff = git::browse::diff_range(bare_repo, base, head).await?;
    // Two-dot: commits reachable from `head` but not `base` — "what `head`
    // adds", matching the three-dot diff above (changes since divergence).
    let range = format!("{base}..{head}");
    let commits = git::browse::log(bare_repo, &range, usize::MAX, None).await?;
    let avatars = commit_avatars(ctx, bare_repo, &commits).await;
    let diff_view = effective_diff_view(headers);
    let return_to = format!(
        "/{}?view=compare&base={}&head={}",
        url_encode(repo_name),
        url_encode(base),
        url_encode(head)
    );

    Ok(html! {
        h3 { (t::compare_heading(locale)) code { (base) } "..." code { (head) } }
        @if commits.is_empty() {
            p class="muted" { (t::no_new_commits(locale)) }
        } @else {
            (commit_table(repo_name, &commits, &avatars, locale))
        }
        @if !stat.trim().is_empty() {
            div class="scroll-x" { pre class="diff" { (stat) } }
        }
        p { (PreEscaped(render_diff_view_buttons(&return_to, diff_view))) }
        @match diff_view {
            DiffView::Unified => div class="scroll-x" { pre class="diff" { (PreEscaped(color_diff(&diff))) } },
            DiffView::Split => div class="scroll-x full-bleed" { (side_by_side_diff(&diff)) },
        }
    }
    .into_string())
}

/// A GET form (linkable/bookmarkable, unlike the settings tab's POST forms)
/// picking base/head from every existing branch/tag — a real `<select>`,
/// not free text: there's no reason to let someone type a ref that isn't
/// one of these, and a dropdown can't typo into an invalid one.
/// `validate_rev` at the handler still gates it either way.
pub(super) fn render_compare_form(repo_name: &str, ref_names: &[&str], locale: Locale) -> Markup {
    let action = format!("/{}", crate::webui::url_encode(repo_name));
    html! {
        h3 { (t::compare_refs_heading(locale)) }
        form method="get" action=(action) {
            input type="hidden" name="view" value="compare";
            (t::base_label(locale)) select name="base" {
                @for name in ref_names {
                    option value=(name) { (name) }
                }
            }
            (t::head_label(locale)) select name="head" {
                @for name in ref_names {
                    option value=(name) { (name) }
                }
            }
            button type="submit" { (t::compare_button(locale)) }
        }
    }
}
