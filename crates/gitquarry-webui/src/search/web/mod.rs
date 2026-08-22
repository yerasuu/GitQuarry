//! `/_search`: cross-repo search — filenames and commit messages across
//! every repo this request can see. Same visibility rule as the index
//! ([`crate::webui::repo::visible_repo_names`]): `Public` always, `Limited`
//! only if logged in. A `Private` repo the caller has an explicit grant on
//! is still reached directly by URL, same as the index — it just doesn't
//! show up in search results either, consistent with that same scope.
//!
//! Two independent primitives fan out per visible repo: [`filenames`]
//! (`git ls-tree -r`) and [`commits`] (`git log --grep`, reusing
//! [`gitquarry_core::git::browse::search_log`], the same primitive the per-repo log
//! view's own search box already uses). Both cap at [`MAX_RESULTS`] total
//! hits, repo-name order, with an explicit truncation notice rather than a
//! silent cut.

#[cfg(any(test, feature = "test-util"))]
pub mod commits;
#[cfg(not(any(test, feature = "test-util")))]
mod commits;
#[cfg(any(test, feature = "test-util"))]
pub mod filenames;
#[cfg(not(any(test, feature = "test-util")))]
mod filenames;

use crate::auth::web::login;
use crate::webui::{AppError, State_, effective_density, effective_locale, effective_theme, layout, url_encode};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use gitquarry_core::app::AppContext;
use maud::{Markup, html};
use std::collections::HashMap;

/// Hard cap on hits shown per section (filenames, commits) — a query broad
/// enough to exceed it gets a "refine your search" notice instead of an
/// unbounded page.
pub(crate) const MAX_RESULTS: usize = 100;

/// How many repos either half of `/_search` scans at once. The scan used to
/// spawn one task per repo with no ceiling, which on a large instance turns
/// a single anonymous `GET /_search?q=a` into thousands of concurrent
/// backend reads and `git` invocations — nothing rate-limits this route
/// (`GitRateLimiter` covers the git transport only), so it was a
/// resource-exhaustion primitive available without a session. Bounded work
/// per request instead: the same repos still get scanned, just a windowful
/// at a time, which is plenty to keep the backend busy without letting one
/// request size itself off the repo count.
pub(super) const MAX_CONCURRENT_REPO_SCANS: usize = 32;

pub(crate) async fn handler_search_get(
    State(ctx): State_,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let q = params.get("q").cloned().unwrap_or_default();
    let topnav = login::topnav(&ctx, &headers).await;

    let results = if q.is_empty() {
        None
    } else {
        gitquarry_core::validate::validate_query(&q).map_err(AppError::bad_request)?;
        let viewer_logged_in = login::current_user(&ctx, &headers).await.is_some();
        let repo_names = crate::webui::repo::visible_repo_names(&ctx, viewer_logged_in).await?;
        let (file_hits, files_truncated) = filenames::search_all(&ctx, &repo_names, &q).await;
        let (commit_hits, commits_truncated) = commits::search_all(&ctx, &repo_names, &q).await;
        Some(SearchResults {
            file_hits,
            files_truncated,
            commit_hits,
            commits_truncated,
        })
    };

    let body = render_search_page(&q, results, locale).into_string();
    Ok(Html(layout(
        "search",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Repo's local mirror path and current head rev, or `None` if the repo no
/// longer resolves, its mirror can't be synced, or it has no commits yet —
/// any of these just means "no hits from this repo", not a search failure.
///
#[cfg(any(test, feature = "test-util"))]
pub async fn mirror_and_head(ctx: &AppContext, repo: &str) -> Option<(std::path::PathBuf, String)> {
    mirror_and_head_impl(ctx, repo).await
}
#[cfg(not(any(test, feature = "test-util")))]
async fn mirror_and_head(ctx: &AppContext, repo: &str) -> Option<(std::path::PathBuf, String)> {
    mirror_and_head_impl(ctx, repo).await
}

/// `Some(uid)` if `repo` names a local repo this bucket tracks — what
/// [`filenames::search_all`]/[`commits::search_all`] key
/// [`gitquarry_core::storage::search_index::get_search_index`] on, falling back to a
/// live git search when this is `None` or no index has been built yet.
async fn repo_uid_if_local(ctx: &AppContext, repo: &str) -> Option<gitquarry_core::storage::uid::Uid> {
    ctx.resolve_repo(repo).await.ok()?.map(|r| r.uid)
}

async fn mirror_and_head_impl(ctx: &AppContext, repo: &str) -> Option<(std::path::PathBuf, String)> {
    let resolved = ctx.resolve_repo(repo).await.ok()??;
    let mirror = ctx.open_mirror(&resolved).await.ok()?;
    let head = gitquarry_core::git::browse::resolve_head(&mirror.path).await.ok()??;
    Some((mirror.path, head))
}

#[cfg(any(test, feature = "test-util"))]
pub struct SearchResults {
    pub file_hits: Vec<filenames::FilenameHit>,
    pub files_truncated: bool,
    pub commit_hits: Vec<commits::CommitHit>,
    pub commits_truncated: bool,
}
#[cfg(not(any(test, feature = "test-util")))]
struct SearchResults {
    file_hits: Vec<filenames::FilenameHit>,
    files_truncated: bool,
    commit_hits: Vec<commits::CommitHit>,
    commits_truncated: bool,
}

#[cfg(any(test, feature = "test-util"))]
pub fn render_search_page(q: &str, results: Option<SearchResults>, locale: gitquarry_i18n::Locale) -> Markup {
    render_search_page_impl(q, results, locale)
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_search_page(q: &str, results: Option<SearchResults>, locale: gitquarry_i18n::Locale) -> Markup {
    render_search_page_impl(q, results, locale)
}

fn render_search_page_impl(q: &str, results: Option<SearchResults>, locale: gitquarry_i18n::Locale) -> Markup {
    use gitquarry_i18n::web::search as t;
    html! {
        p class="muted" { (t::search_intro(locale)) }
        form method="get" action="/_search" {
            input name="q" value=(q) placeholder=(t::search_placeholder(locale));
            button type="submit" { (t::search_button(locale)) }
        }
        @if let Some(results) = results {
            h4 { (t::filenames_heading(locale)) }
            @if results.file_hits.is_empty() {
                p { em { (t::no_filename_matches(locale)) } }
            } @else {
                table class="list" {
                    tr { th { (t::table_header_repo(locale)) } th { (t::table_header_path(locale)) } }
                    @for hit in &results.file_hits {
                        tr {
                            td { a href=(format!("/{}", url_encode(&hit.repo))) { (hit.repo) } }
                            td { code { a href=(crate::webui::repo_link(&hit.repo, "blob", &hit.rev).path(&hit.path).to_string()) { (hit.path) } } }
                        }
                    }
                }
                @if results.files_truncated {
                    p { em { (t::results_truncated(locale, MAX_RESULTS)) } }
                }
            }
            h4 { (t::commits_heading(locale)) }
            @if results.commit_hits.is_empty() {
                p { em { (t::no_commit_matches(locale)) } }
            } @else {
                table class="list" {
                    tr { th { (t::table_header_repo(locale)) } th { (t::table_header_commit(locale)) } }
                    @for hit in &results.commit_hits {
                        tr {
                            td { a href=(format!("/{}", url_encode(&hit.repo))) { (hit.repo) } }
                            td { a href=(crate::webui::repo_link(&hit.repo, "commit", "").sha(&hit.sha).to_string()) { (hit.subject) } }
                        }
                    }
                }
                @if results.commits_truncated {
                    p { em { (t::results_truncated(locale, MAX_RESULTS)) } }
                }
            }
        }
    }
}
