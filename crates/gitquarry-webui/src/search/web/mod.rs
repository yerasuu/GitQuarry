//! `/_search`: cross-repo search — filenames and commit messages across
//! every repo this request can see. Same visibility rule as the index
//! ([`crate::webui::repo::visible_searchable_repos`]): `Public` always, `Limited`
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
//!
//! Federation layer 3 (`docs/federation-concept.md` capa 3) extends both
//! halves without a third: [`filenames`]/[`commits`] already treat any repo
//! candidate as "just a bare repo at some path", so a linked peer's repo
//! whose mirror already exists under `_remote/<peer>/<repo>` (because
//! someone browsed or forked it before — see [`crate::webui::remote_repo::handler_remote_repo`])
//! rides the exact same two primitives, listed alongside local candidates
//! by [`list_synced_peer_repos`]. Never triggers a new `git fetch` on its
//! own — a peer repo nobody has opened yet simply isn't a candidate, same
//! "search reads what's already there, never syncs on demand" rule
//! [`mirror_and_head`] enforces for the peer branch. Metadata search (name/
//! description/topics, via [`search_peer_metadata`]) is `[]` unless
//! `[federation]` is configured, otherwise a live call to that separate
//! process (`docs/federation-binary-split-concept.md` section 8) — no repo
//! candidate list involved at all either way.

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
        let repos = crate::webui::repo::visible_searchable_repos(&ctx, viewer_logged_in).await?;
        // A hit only carries the repo's `full_name` (that's what the scan
        // was keyed on, and what its links need), so the labels the rows
        // render come back alongside it as a lookup rather than per hit.
        let mut repo_names = Vec::with_capacity(repos.len());
        let mut labels = HashMap::with_capacity(repos.len());
        for (name, label) in repos {
            repo_names.push(name.clone());
            labels.insert(name, label);
        }
        // Federation layer 3: a linked peer's repo whose mirror already
        // exists locally rides the same scan — no label lookup for these,
        // `labels.get`'s own fallback below just shows their `_remote/...`
        // path as-is.
        repo_names.extend(list_synced_peer_repos(&ctx).await);
        let (file_hits, files_truncated) = filenames::search_all(&ctx, &repo_names, &q).await;
        let (commit_hits, commits_truncated) = commits::search_all(&ctx, &repo_names, &q).await;
        let peer_metadata_hits = search_peer_metadata(&ctx, &q).await;
        Some(SearchResults {
            file_hits,
            files_truncated,
            commit_hits,
            commits_truncated,
            peer_metadata_hits,
            labels,
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
/// A `repo` starting with `_remote/` (from [`list_synced_peer_repos`]) is a
/// peer's already-synced mirror: read directly off disk under the same
/// per-repo lock [`gitquarry_core::app::AppContext::open_remote_mirror`] itself takes
/// (so a concurrent live browse mid-fetch can't be read half-written), but
/// deliberately never through `open_remote_mirror` — that would sync
/// (`git fetch`) on every search, which is exactly the "a text query must
/// never turn into a fetch against some peer" rule
/// `docs/federation-concept.md` capa 3 calls out.
#[cfg(any(test, feature = "test-util"))]
pub async fn mirror_and_head(ctx: &AppContext, repo: &str) -> Option<(std::path::PathBuf, String)> {
    mirror_and_head_impl(ctx, repo).await
}
#[cfg(not(any(test, feature = "test-util")))]
async fn mirror_and_head(ctx: &AppContext, repo: &str) -> Option<(std::path::PathBuf, String)> {
    mirror_and_head_impl(ctx, repo).await
}

/// `Some(uid)` if `repo` names a local repo this bucket tracks (never true
/// for a `"_remote/..."` peer mirror, which has no `RepoConfig`/uid of its
/// own) — what [`filenames::search_all`]/[`commits::search_all`] key
/// [`gitquarry_core::storage::search_index::get_search_index`] on, falling back to a
/// live git search when this is `None` or no index has been built yet.
async fn repo_uid_if_local(ctx: &AppContext, repo: &str) -> Option<gitquarry_core::storage::uid::Uid> {
    if repo.starts_with("_remote/") {
        return None;
    }
    ctx.resolve_repo(repo).await.ok()?.map(|r| r.uid)
}

async fn mirror_and_head_impl(ctx: &AppContext, repo: &str) -> Option<(std::path::PathBuf, String)> {
    if repo.starts_with("_remote/") {
        let _guard = ctx.locks.lock(repo).await;
        let path = ctx.mirrors_root.join(repo);
        if !tokio::fs::try_exists(path.join("HEAD")).await.unwrap_or(false) {
            return None;
        }
        let head = gitquarry_core::git::browse::resolve_head(&path).await.ok()??;
        return Some((path, head));
    }
    let resolved = ctx.resolve_repo(repo).await.ok()??;
    let mirror = ctx.open_mirror_read(&resolved).await.ok()?;
    let head = gitquarry_core::git::browse::resolve_head(&mirror.path).await.ok()??;
    Some((mirror.path, head))
}

/// Every peer mirror already synced under `_remote/<peer>/<repo>` — same
/// bare-repo-marker (`HEAD` file) walk `mirror::gc::list_mirrors` uses to
/// find any mirror, just scoped to the `_remote/` subtree and not shared
/// with `gc` (that one also needs size/mtime for eviction, this only needs
/// the relative path). Returns `"_remote/<peer>/<repo...>"` strings, ready
/// to hand straight to [`mirror_and_head`].
#[cfg(any(test, feature = "test-util"))]
pub async fn list_synced_peer_repos(ctx: &AppContext) -> Vec<String> {
    list_synced_peer_repos_impl(ctx).await
}
#[cfg(not(any(test, feature = "test-util")))]
async fn list_synced_peer_repos(ctx: &AppContext) -> Vec<String> {
    list_synced_peer_repos_impl(ctx).await
}

async fn list_synced_peer_repos_impl(ctx: &AppContext) -> Vec<String> {
    let root = ctx.mirrors_root.join("_remote");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        if tokio::fs::try_exists(dir.join("HEAD")).await.unwrap_or(false) {
            if let Ok(rel) = dir.strip_prefix(&ctx.mirrors_root) {
                out.push(rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"));
            }
            continue;
        }
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.is_ok_and(|ft| ft.is_dir()) {
                stack.push(entry.path());
            }
        }
    }
    out
}

#[cfg(any(test, feature = "test-util"))]
pub struct PeerMetadataHit {
    pub peer: String,
    pub name: String,
    pub description: Option<String>,
}
#[cfg(not(any(test, feature = "test-util")))]
pub(super) struct PeerMetadataHit {
    peer: String,
    name: String,
    description: Option<String>,
}

/// Substring match on name/description/topics against every linked peer's
/// repo list — `[]` if `[federation]` isn't configured, otherwise a live
/// `GET /_internal-out/peer-repos` call against the separate
/// `gitquarry-federation` process (`docs/federation-binary-split-concept.md`
/// section 8). A repo nobody has browsed/forked yet (so no local mirror
/// exists) still shows up here — content search is what needs the mirror,
/// metadata search doesn't.
async fn search_peer_metadata(ctx: &AppContext, q: &str) -> Vec<PeerMetadataHit> {
    let entries = match &ctx.federation {
        Some(federation) => crate::webui::federation_client::peer_repos(federation).await,
        None => Vec::new(),
    };
    let q_lower = q.to_ascii_lowercase();
    let mut hits = Vec::new();
    for (peer, repos) in entries {
        for r in repos {
            let matches = r.name.to_ascii_lowercase().contains(&q_lower)
                || r.description
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&q_lower)
                || r.topics.iter().any(|t| t.to_ascii_lowercase().contains(&q_lower));
            if matches {
                hits.push(PeerMetadataHit {
                    peer: peer.clone(),
                    name: r.name,
                    description: r.description,
                });
            }
        }
    }
    hits.sort_by(|a, b| (&a.peer, &a.name).cmp(&(&b.peer, &b.name)));
    hits
}

/// `"_remote/<peer>/<repo>"` reads as `"<peer>/<repo> (remote)"` — same
/// cosmetic trim the `/` index's own federated section does, everywhere a
/// search hit's repo name is shown as text rather than built into an `href`
/// (`url_encode` already leaves `/` alone, so the raw `_remote/...` string
/// is a perfectly correct link target as-is, just an ugly label).
#[cfg(any(test, feature = "test-util"))]
pub fn display_repo(repo: &str) -> String {
    display_repo_impl(repo)
}
#[cfg(not(any(test, feature = "test-util")))]
fn display_repo(repo: &str) -> String {
    display_repo_impl(repo)
}

fn display_repo_impl(repo: &str) -> String {
    match repo.strip_prefix("_remote/") {
        Some(rest) => format!("{rest} (remote)"),
        None => repo.to_string(),
    }
}

#[cfg(any(test, feature = "test-util"))]
pub struct SearchResults {
    pub file_hits: Vec<filenames::FilenameHit>,
    pub files_truncated: bool,
    pub commit_hits: Vec<commits::CommitHit>,
    pub commits_truncated: bool,
    pub peer_metadata_hits: Vec<PeerMetadataHit>,
    /// `full_name` -> the label its rows render (absent means "render the
    /// name as-is"). Built once per request instead of per hit: one repo
    /// answers many hits.
    pub labels: HashMap<String, String>,
}
#[cfg(not(any(test, feature = "test-util")))]
struct SearchResults {
    file_hits: Vec<filenames::FilenameHit>,
    files_truncated: bool,
    commit_hits: Vec<commits::CommitHit>,
    commits_truncated: bool,
    peer_metadata_hits: Vec<PeerMetadataHit>,
    labels: HashMap<String, String>,
}

#[cfg(any(test, feature = "test-util"))]
pub fn render_search_page(q: &str, results: Option<SearchResults>, locale: gitquarry_i18n::Locale) -> Markup {
    render_search_page_impl(q, results, locale)
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_search_page(q: &str, results: Option<SearchResults>, locale: gitquarry_i18n::Locale) -> Markup {
    render_search_page_impl(q, results, locale)
}

/// A hit's row label out of [`SearchResults::labels`] — the raw name for a
/// repo missing from it (no `display_name`, or one added between the scan
/// and this render).
fn repo_label<'a>(labels: &'a HashMap<String, String>, name: &'a str) -> &'a str {
    labels.get(name).map(String::as_str).unwrap_or(name)
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
                            td { a href=(format!("/{}", url_encode(&hit.repo))) { (display_repo(repo_label(&results.labels, &hit.repo))) } }
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
                            td { a href=(format!("/{}", url_encode(&hit.repo))) { (display_repo(repo_label(&results.labels, &hit.repo))) } }
                            td { a href=(crate::webui::repo_link(&hit.repo, "commit", "").sha(&hit.sha).to_string()) { (hit.subject) } }
                        }
                    }
                }
                @if results.commits_truncated {
                    p { em { (t::results_truncated(locale, MAX_RESULTS)) } }
                }
            }
            @if !results.peer_metadata_hits.is_empty() {
                h4 { (t::remote_repos_heading(locale)) }
                p class="muted" { em { (t::remote_repos_hint(locale)) } }
                table class="list" {
                    tr { th { (t::table_header_repo(locale)) } th { (t::table_header_description(locale)) } }
                    @for hit in &results.peer_metadata_hits {
                        tr {
                            td { a href=(format!("/_remote/{}/{}", url_encode(&hit.peer), url_encode(&hit.name))) { (hit.name) " (" (hit.peer) ")" } }
                            td { (hit.description.clone().unwrap_or_default()) }
                        }
                    }
                }
            }
        }
    }
}
