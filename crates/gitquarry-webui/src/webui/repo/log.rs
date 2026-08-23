//! The `log` tab: paginated commit history, optionally filtered by
//! `?q=` (passed to `git log --grep`).

use super::LOG_PAGE_SIZE;
use super::components::{commit_avatars, commit_table_with_stats};
use crate::webui::{breadcrumbs, repo_link, url_encode};
use anyhow::Result;
use gitquarry_core::app::AppContext;
use gitquarry_core::git;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::log as t;
use maud::{PreEscaped, html};
use std::path::Path as FsPath;

#[allow(clippy::too_many_arguments)]
pub async fn render_log(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    display_name: Option<&str>,
    rev: &str,
    path: &str,
    q: Option<&str>,
    skip: usize,
    locale: Locale,
) -> Result<String> {
    let commits = match q {
        Some(q) => git::browse::search_log_with_stats(bare_repo, rev, q, LOG_PAGE_SIZE, Some(path)).await?,
        None => git::browse::log_page_with_stats(bare_repo, rev, LOG_PAGE_SIZE, skip, Some(path)).await?,
    };
    let commit_infos: Vec<_> = commits.iter().map(|(c, _)| c.clone()).collect();
    let avatars = commit_avatars(ctx, bare_repo, &commit_infos).await;

    // Pagination only applies to the unfiltered log: a search's total match
    // count isn't known without a second, more expensive query, and result
    // sets from a text search are small enough in practice not to need it.
    let total = if q.is_none() {
        Some(git::browse::commit_count(bare_repo, rev, Some(path)).await?)
    } else {
        None
    };

    Ok(html! {
        // Only when scoped to a subdirectory: at the repo root, `breadcrumbs`
        // still renders one crumb (the repo name, linking back to `tree`) —
        // redundant here, the repo's already named in the page header right
        // above this tab.
        @if !path.is_empty() {
            (PreEscaped(breadcrumbs(repo_name, display_name, rev, path, "tree", locale)))
        }
        form method="get" action=(format!("/{}", url_encode(repo_name))) {
            input type="hidden" name="view" value="log";
            input type="hidden" name="ref" value=(rev);
            input type="hidden" name="path" value=(path);
            input name="q" value=(q.unwrap_or("")) placeholder=(t::search_commit_messages_placeholder(locale));
            button type="submit" { (t::search_button(locale)) }
        }
        (commit_table_with_stats(repo_name, &commits, &avatars, locale))
        @if let Some(total) = total {
            ul class="pagination" {
                @if skip > 0 {
                    @let prev = skip.saturating_sub(LOG_PAGE_SIZE);
                    li class="page-item" { a href=(repo_link(repo_name, "log", rev).path(path).skip(prev).to_string()) { (t::newer_link(locale)) } }
                } @else {
                    li class="page-item disabled" { (t::newer_link(locale)) }
                }
                @if skip + LOG_PAGE_SIZE < total {
                    li class="page-item" { a href=(repo_link(repo_name, "log", rev).path(path).skip(skip + LOG_PAGE_SIZE).to_string()) { (t::older_link(locale)) } }
                } @else {
                    li class="page-item disabled" { (t::older_link(locale)) }
                }
            }
        }
    }
    .into_string())
}
