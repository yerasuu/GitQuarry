//! The repo index and every HTML browsing view: summary, refs, tree, blob,
//! log, commit, stats, blame. Views that don't return an HTML page (plain
//! blob bytes, patch, rawdiff, snapshot, atom, release assets) live on their
//! own route tree instead — see [`super::raw`].
//!
//! Split one file per view — [`components`] holds render helpers 2+ views
//! share (a table of refs, a table of commits); anything used by exactly one
//! view lives in that view's own file. This file is the two entry points:
//! the repo index ([`handler_index`]) and the per-repo dispatcher
//! ([`handler_repo`]) that picks a view by `?view=` and calls its `render_*`.

mod activity;
pub(crate) mod blame;
pub(crate) mod blob;
pub(crate) mod branches;
#[cfg(any(test, feature = "test-util"))]
pub mod commit;
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) mod commit;
mod compare;
#[cfg(any(test, feature = "test-util"))]
pub mod components;
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) mod components;
mod diff;
pub(crate) mod log;
#[cfg(any(test, feature = "test-util"))]
pub mod merges;
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) mod merges;
#[cfg(any(test, feature = "test-util"))]
pub mod refs;
#[cfg(not(any(test, feature = "test-util")))]
mod refs;
#[cfg(any(test, feature = "test-util"))]
pub mod releases;
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) mod releases;
#[cfg(any(test, feature = "test-util"))]
pub mod stats;
#[cfg(not(any(test, feature = "test-util")))]
mod stats;
#[cfg(any(test, feature = "test-util"))]
pub mod summary;
#[cfg(not(any(test, feature = "test-util")))]
mod summary;
pub(crate) use summary::community_stats_html;
pub(crate) mod tags;
pub(crate) mod tree;

use activity::render_activity;
#[cfg(any(test, feature = "test-util"))]
pub use blame::render_blame;
#[cfg(not(any(test, feature = "test-util")))]
use blame::render_blame;
#[cfg(any(test, feature = "test-util"))]
pub use blob::render_blob;
#[cfg(not(any(test, feature = "test-util")))]
use blob::render_blob;
#[cfg(any(test, feature = "test-util"))]
pub use commit::render_commit;
#[cfg(not(any(test, feature = "test-util")))]
use commit::render_commit;
use compare::render_compare;
#[cfg(any(test, feature = "test-util"))]
pub use log::render_log;
#[cfg(not(any(test, feature = "test-util")))]
use log::render_log;
use merges::{NewMergeFormValues, render_merge_detail, render_merges_list, render_new_merge_form};
#[cfg(any(test, feature = "test-util"))]
pub use refs::render_refs;
#[cfg(not(any(test, feature = "test-util")))]
use refs::render_refs;
use releases::{render_release_detail, render_releases_list};
#[cfg(any(test, feature = "test-util"))]
pub use stats::render_stats;
#[cfg(not(any(test, feature = "test-util")))]
use stats::render_stats;
use summary::{render_about, render_clone_urls, render_language_bar, render_summary};
#[cfg(any(test, feature = "test-util"))]
pub use tree::render_tree;
#[cfg(not(any(test, feature = "test-util")))]
use tree::render_tree;

use super::{
    AppError, HeaderExtras, Page, State_, compound_path, effective_density, effective_locale, effective_theme,
    header_bar, layout, render_repo_action_buttons, render_text_size_buttons, repo_label, repo_link, repo_settings,
    url_encode,
};
use crate::auth::web::login;
use crate::settings::web::require_admin;
use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::{domain, git, storage, validate};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::task::JoinSet;

/// How many commits get listed under "recent commits" (tree/summary).
pub(crate) const RECENT_LOG_LIMIT: usize = 10;
/// Page size for the log tab and for the stats "commits per month" window.
const LOG_PAGE_SIZE: usize = 50;
const STATS_LOG_LIMIT: usize = 500;
/// How many branches/tags show on the summary tab before "see all".
pub(crate) const SUMMARY_REFS_LIMIT: usize = 5;
/// A blob bigger than this doesn't get rendered inline: pulling it into
/// memory and highlighting/escaping it multiplies its size several times
/// over. `export::plain` has its own, larger cap for raw downloads.
const MAX_BLOB_BYTES: u64 = 1 << 20;
/// How much of a commit's patch the commit page renders before handing the
/// reader the raw patch instead. Same reasoning as [`MAX_BLOB_BYTES`] and
/// the same order of magnitude — escaping and colorizing a diff several
/// times its own size, then asking a browser to lay it out, is the cost
/// being bounded, not git's own work. `export::patch`/`export::rawdiff`
/// stay uncapped.
const MAX_DIFF_BYTES: u64 = 512 << 10;

/// One index row's worth of `RepoConfig`, not the whole thing: the index
/// only ever needs these three, and threading the full config through would
/// invite reaching for fields (owner, description) that belong on the
/// repo's own page, not the list of all of them.
struct IndexRepo {
    name: String,
    /// What every row renders ([`crate::webui::repo_label::repo_label`]),
    /// while `name` above stays the string the row's href is built from.
    /// Derived once here rather than per render site: this page renders each
    /// row twice (cards and the compact table), sorts and filters on it too.
    label: String,
    description: Option<String>,
    archived: bool,
    topics: std::collections::BTreeSet<String>,
    star_count: u32,
    primary_language: Option<String>,
    last_push: Option<u64>,
    last_push_commits: Option<u32>,
    last_push_branch: Option<String>,
}

/// Compact stamp ("2h", "3d") for the sidebar's own `<time>` column and the
/// compact table's dense `.rl-meta` row, both too tight for full words —
/// coarse, single-unit buckets rather than a full duration breakdown.
/// Unlocalized like the rest of this fragment already is (`results`, below,
/// has no `locale` in scope — see its own doc comment): threading one
/// through just for this string would be the only i18n on an otherwise-
/// English page.
#[cfg(any(test, feature = "test-util"))]
pub fn relative_time_ago(then: u64, now: u64) -> String {
    relative_time_ago_impl(then, now)
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn relative_time_ago(then: u64, now: u64) -> String {
    relative_time_ago_impl(then, now)
}
fn relative_time_ago_impl(then: u64, now: u64) -> String {
    let diff = now.saturating_sub(then);
    if diff < 60 {
        return "just now".to_string();
    }
    let (value, unit) = if diff < 3600 {
        (diff / 60, "m")
    } else if diff < 86400 {
        (diff / 3600, "h")
    } else if diff < 604_800 {
        (diff / 86400, "d")
    } else if diff < 2_629_800 {
        (diff / 604_800, "w")
    } else if diff < 31_557_600 {
        (diff / 2_629_800, "mo")
    } else {
        (diff / 31_557_600, "y")
    };
    format!("{value}{unit} ago")
}

/// Full-words form ("updated 3 hours ago", "updated yesterday") for the
/// repo card's own meta line — that line has room `relative_time_ago`'s
/// compact stamp doesn't need to save, and reads better spelled out next to
/// the language name and star count it sits beside.
#[cfg(any(test, feature = "test-util"))]
pub fn relative_time_words(then: u64, now: u64) -> String {
    relative_time_words_impl(then, now)
}
#[cfg(not(any(test, feature = "test-util")))]
fn relative_time_words(then: u64, now: u64) -> String {
    relative_time_words_impl(then, now)
}
fn relative_time_words_impl(then: u64, now: u64) -> String {
    let diff = now.saturating_sub(then);
    fn plural(n: u64, unit: &str) -> String {
        format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
    }
    if diff < 60 {
        return "just now".to_string();
    }
    if diff < 3600 {
        return plural(diff / 60, "minute");
    }
    if diff < 86400 {
        return plural(diff / 3600, "hour");
    }
    if diff < 172_800 {
        return "yesterday".to_string();
    }
    if diff < 604_800 {
        return plural(diff / 86400, "day");
    }
    if diff < 2_629_800 {
        return plural(diff / 604_800, "week");
    }
    if diff < 31_557_600 {
        return plural(diff / 2_629_800, "month");
    }
    plural(diff / 31_557_600, "year")
}

/// One entry in the home page's "recent activity" sidebar — deliberately
/// only these three: a push to a repo's default branch (`IndexRepo::last_push`
/// et al, never `storage::audit`, see that field's own doc comment), a
/// release, or a merged MR. All three read as "important code changed" the
/// way GitHub's own dashboard activity does; everything else `storage::audit`
/// tracks (config saves, permission grants, star/watch toggles, ...) is
/// exactly what a repo's own `?view=activity` page deliberately dropped —
/// pulling it back in here just because this widget also touches `audit`
/// for release/MR entries would undo that.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityKind {
    Push { commits: u32, branch: Option<String> },
    Release { tag: String },
    MrMerged { number: u64 },
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActivityKind {
    Push { commits: u32, branch: Option<String> },
    Release { tag: String },
    MrMerged { number: u64 },
}

impl ActivityKind {
    pub fn describe(&self) -> String {
        match self {
            ActivityKind::Push { commits, branch } => {
                let plural = if *commits == 1 { "" } else { "s" };
                match branch {
                    Some(b) => format!("{commits} commit{plural} pushed to {b}"),
                    None => format!("{commits} commit{plural} pushed"),
                }
            }
            ActivityKind::Release { tag } => format!("tagged {tag}"),
            ActivityKind::MrMerged { number } => format!("MR #{number} merged"),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub repo: String,
    pub at: u64,
    pub kind: ActivityKind,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivityItem {
    repo: String,
    at: u64,
    kind: ActivityKind,
}

/// Newest-first, capped at `limit` — the merge-sort half of the sidebar,
/// split out from the audit-reading/`IndexRepo`-scanning half above it so
/// it's testable without either.
#[cfg(any(test, feature = "test-util"))]
pub fn top_recent_activity(items: Vec<ActivityItem>, limit: usize) -> Vec<ActivityItem> {
    top_recent_activity_impl(items, limit)
}
#[cfg(not(any(test, feature = "test-util")))]
fn top_recent_activity(items: Vec<ActivityItem>, limit: usize) -> Vec<ActivityItem> {
    top_recent_activity_impl(items, limit)
}
fn top_recent_activity_impl(mut items: Vec<ActivityItem>, limit: usize) -> Vec<ActivityItem> {
    items.sort_by_key(|i| std::cmp::Reverse(i.at));
    items.truncate(limit);
    items
}

pub(crate) async fn handler_index(
    State(ctx): State_,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Page {
    let viewer_logged_in = login::current_user(&ctx, &headers).await.is_some();
    let mut repos = list_visible_repos(&ctx, viewer_logged_in).await?;
    // Sorted on the label, not the slug: the rows below are ordered by what
    // the reader can actually see. Namespaces stay contiguous either way —
    // a label keeps its `<namespace>/` prefix.
    repos.sort_by(|a, b| a.label.cmp(&b.label));
    // Both stay unaffected by `?q=` below: they describe the whole visible
    // set, not the current search/filter.
    let repo_count = repos.len();
    let ns_count = repos
        .iter()
        .filter_map(|r| r.name.rsplit_once('/').map(|(ns, _)| ns))
        .collect::<HashSet<_>>()
        .len();

    let q = params.get("q").map(String::as_str).unwrap_or("");
    let q_lower = q.to_ascii_lowercase();
    // A topic badge links to `?q=<topic>`, so the same search box doubles
    // as a topic filter without a second query param/UI to maintain.
    let filtered: Vec<&IndexRepo> = repos
        .iter()
        .filter(|r| {
            q.is_empty()
                || r.name.to_ascii_lowercase().contains(&q_lower)
                // Matched on the label too, not just the slug — otherwise
                // typing what's on screen finds nothing.
                || r.label.to_ascii_lowercase().contains(&q_lower)
                || r.topics.iter().any(|t| t.contains(&q_lower))
        })
        .collect();

    // Repos are sorted, so a namespace (everything before the first `/`)
    // groups into a contiguous run: emit a header row each time it changes.
    // Repos with no `/` have no namespace and never get a header.
    let mut last_ns: Option<&str> = None;
    let rows: Vec<(Option<&str>, &IndexRepo)> = filtered
        .iter()
        .map(|repo| {
            let ns = repo.name.rsplit_once('/').map(|(ns, _)| ns);
            let header = if ns != last_ns { ns } else { None };
            last_ns = ns;
            (header, *repo)
        })
        .collect();

    // Per-card ssh clone hint (cards view only) — same host/port derivation
    // `summary::render_clone_urls` uses for a single repo's own clone table,
    // just inlined here instead of reused: that helper renders a whole
    // `<h3>+table>`, not a bare string, so there's nothing smaller to call.
    let ssh_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.split(':').next())
        .unwrap_or("<host>");
    let ssh_port = ctx.ssh_bind.rsplit_once(':').map(|(_, p)| p).unwrap_or(&ctx.ssh_bind);
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // `#repo-list`'s own inner content, factored out so the async path below
    // (live search-as-you-type, `style/app.js`'s `data-live-search`) can
    // return just this fragment instead of the whole page — same
    // `ASYNC_HEADER` convention the toggle forms use, just for a plain GET
    // instead of a form POST.
    let results = maud::html! {
        @if filtered.is_empty() {
            p { em { "(empty, nothing pushed yet)" } }
        }
        div class="repo-cards" {
            @for (header, repo) in &rows {
                @if let Some(ns) = header {
                    p class="repo-cards-ns" { (ns) }
                }
                div class="card card-pad repo-card" {
                    div class="repo-card-title" {
                        a href=(format!("/{}", url_encode(&repo.name))) { (repo.label) }
                        @if repo.archived {
                            " " span class="badge" { "archived" }
                        }
                        (maud::PreEscaped(super::topic_badges(&repo.topics)))
                        code class="clone-url" { "ssh://" (ssh_host) ":" (ssh_port) "/" (repo.name) }
                    }
                    @if let Some(desc) = &repo.description {
                        @if !desc.is_empty() {
                            p class="repo-card-desc" { (desc) }
                        }
                    }
                    @if repo.star_count > 0 || repo.primary_language.is_some() || repo.last_push.is_some() {
                        div class="repo-card-meta" {
                            @if let Some(lang) = &repo.primary_language {
                                span {
                                    span class=(format!("lang-swatch {}", summary::language_class(lang))) {}
                                    (lang)
                                }
                            }
                            @if repo.star_count > 0 {
                                span { (repo.star_count) " \u{2605}" }
                            }
                            @if let Some(last_push) = repo.last_push {
                                span { "updated " (relative_time_words(last_push, now_unix)) }
                            }
                        }
                    }
                }
            }
        }
        // Same rows, cgit-flavored dense table instead of cards — CSS-only
        // swap (`base.css`'s `[data-density='airy']` rule), same pattern as
        // `.header-lang`/`.sidebar-lang` already use for this axis: both
        // markups always render, `data-density` on `<html>` picks which one
        // is visible, no JS needed to switch view when the toggle fires.
        div class="repo-list-compact" {
            @for (header, repo) in &rows {
                @if let Some(ns) = header {
                    p class="repo-cards-ns" { (ns) }
                }
                div class="rl-row" {
                    div class="rl-name" {
                        a href=(format!("/{}", url_encode(&repo.name))) { (repo.label) }
                        @if repo.archived {
                            " " span class="badge" { "archived" }
                        }
                    }
                    div class="rl-desc" {
                        @if let Some(desc) = &repo.description {
                            @if !desc.is_empty() {
                                (desc)
                            }
                        }
                    }
                    div class="rl-topics" { (maud::PreEscaped(super::topic_badges(&repo.topics))) }
                    div class="rl-meta" {
                        @if let Some(lang) = &repo.primary_language {
                            span {
                                span class=(format!("lang-swatch {}", summary::language_class(lang))) {}
                                (lang)
                            }
                        }
                        @if repo.star_count > 0 {
                            span { (repo.star_count) " \u{2605}" }
                        }
                        @if let Some(last_push) = repo.last_push {
                            span class="muted" { (relative_time_ago(last_push, now_unix)) }
                        }
                    }
                }
            }
        }
    };
    if headers.contains_key(super::ASYNC_HEADER) {
        return Ok(Html(results.into_string()));
    }
    // Chrome around `results` that only a full page load needs — skipped on
    // the live-search fragment above, same reasoning `results` itself is
    // factored out for: no point re-fetching the user count or walking the
    // audit log on every keystroke.
    let user_count = access::list_usernames(ctx.backend().as_ref())
        .await
        .map(|names| names.len())
        .unwrap_or(0);
    // Pushes come from each repo's own `last_push*` fields (see
    // `ActivityKind`'s own doc comment for why, never generic `storage::audit`
    // noise); releases and merged MRs are the two `storage::audit` actions
    // narrow and code-relevant enough to earn a deliberate exception. Same
    // visibility `repos` already has from `list_visible_repos` above, so
    // this can never surface a repo the viewer couldn't otherwise see.
    let visible_names: HashSet<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    let mut activity: Vec<ActivityItem> = repos
        .iter()
        .filter_map(|r| {
            Some(ActivityItem {
                repo: r.name.clone(),
                at: r.last_push?,
                kind: ActivityKind::Push {
                    commits: r.last_push_commits.unwrap_or(0),
                    branch: r.last_push_branch.clone(),
                },
            })
        })
        .collect();
    for e in storage::audit::list_recent(&ctx.backend(), &ctx.cipher)
        .await
        .unwrap_or_default()
    {
        let Some(at) = gitquarry_mirror_external::stamp::to_unix(&e.at).and_then(|s| u64::try_from(s).ok()) else {
            continue;
        };
        match e.action.as_str() {
            "release.create" => {
                if let Some((repo, tag)) = e.target.rsplit_once('@')
                    && visible_names.contains(repo)
                {
                    activity.push(ActivityItem {
                        repo: repo.to_string(),
                        at,
                        kind: ActivityKind::Release { tag: tag.to_string() },
                    });
                }
            }
            "merge_request.merge" => {
                if let Some((repo, number)) = e.target.rsplit_once('#')
                    && visible_names.contains(repo)
                    && let Ok(number) = number.parse::<u64>()
                {
                    activity.push(ActivityItem {
                        repo: repo.to_string(),
                        at,
                        kind: ActivityKind::MrMerged { number },
                    });
                }
            }
            _ => {}
        }
    }
    let recent_activity = top_recent_activity(activity, 5);
    // `ActivityItem::repo` is a `full_name` (it's also built from audit
    // targets, which only ever carry that), so the sidebar looks its label
    // back up here rather than carrying one per item.
    let labels: HashMap<&str, &str> = repos.iter().map(|r| (r.name.as_str(), r.label.as_str())).collect();
    let all_topics: std::collections::BTreeSet<String> = repos.iter().flat_map(|r| r.topics.iter().cloned()).collect();
    let body = maud::html! {
        div class="stats-strip" {
            div class="stat" { div class="n" { (repo_count) } div class="l" { "repositories" } }
            div class="stat" { div class="n" { (ns_count) } div class="l" { "namespaces" } }
            div class="stat" { div class="n" { (user_count) } div class="l" { "users" } }
            div class="stat" { div class="n" { "v" (env!("CARGO_PKG_VERSION")) } div class="l" { "server" } }
        }
        div class="repo-layout" {
            div class="repo-main" {
                div class="controls-row" {
                    form method="get" action="/" data-async data-live-search="#repo-list" {
                        input name="q" value=(q) placeholder="search repos or topics";
                        button type="submit" { "search" }
                    }
                    a class="btn" href="/_new" { "+ new repo" }
                }
                p class="muted" { a href="/_search" { "search filenames and commits across every repo" } }
                div id="repo-list" { (results) }
            }
            aside class="repo-sidebar" {
                @if !recent_activity.is_empty() {
                    div class="card card-pad sidebar-box" {
                        h4 class="sidebar-box-title" { "recent activity" }
                        ul class="index-activity" {
                            @for item in &recent_activity {
                                li {
                                    time { (relative_time_ago(item.at, now_unix)) }
                                    div {
                                        a href=(format!("/{}", url_encode(&item.repo))) { (labels.get(item.repo.as_str()).copied().unwrap_or(item.repo.as_str())) }
                                        " " span class="verb" { "— " (item.kind.describe()) }
                                    }
                                }
                            }
                        }
                    }
                }
                @if !all_topics.is_empty() {
                    div class="card card-pad sidebar-box" {
                        h4 class="sidebar-box-title" { "browse by topic" }
                        div class="index-topics" { (maud::PreEscaped(super::topic_badges(&all_topics))) }
                    }
                }
            }
        }
    }
    .into_string();
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    Ok(Html(layout(
        &ctx.bucket,
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    )))
}

/// Repos visible to the anonymous-index-viewer or a logged-in-but-
/// otherwise-unprivileged one: `Public` always, `Limited` too if
/// `viewer_logged_in` — anyone with a *specific* grant on a `Private` repo
/// still reaches it directly by URL, this is just what the index lists.
/// `archived`/`topics`/`star_count` ride along so the index can badge/filter/
/// sort without a second fetch — `star_count` in particular used to mean a
/// whole extra `access.json` fetch shared across every row; now it's just
/// another field on the same per-repo config already being fetched here.
/// Candidate uids come from `list_visible_repo_uids`
/// (`.conf/index/repo-visible/...`), not a bucket-wide scan — a `Private`
/// repo's config is never even fetched here now, instead of being fetched
/// on every anonymous home-page hit just to be filtered back out. Still one
/// config fetch per *candidate* repo, run concurrently, and still a real
/// re-check of `config.visibility` below: the index is a hint (a repo just
/// turned `Private` might still be in it until the next config write
/// self-heals it), never trusted on its own for a privacy decision.
async fn list_visible_repos(ctx: &AppContext, viewer_logged_in: bool) -> Result<Vec<IndexRepo>> {
    // Short-TTL-cached (`AppContext::visible_repo_uids`'s own doc comment):
    // this is a `list_prefix` call, which `CachedBackend` never caches, so
    // it used to hit the backend fresh on every single index/search render.
    let uids = ctx.visible_repo_uids().await?;
    let mut set = JoinSet::new();
    for uid in uids {
        let backend = ctx.backend();
        let cipher = Arc::clone(&ctx.cipher);
        set.spawn(async move {
            // A fetch error hides the repo rather than showing it: fail
            // closed on a privacy check, not open.
            let (config, etag) = storage::settings::get_repo_config(backend.as_ref(), uid.as_str(), &cipher).await?;
            // First uid-based load of a repo predating `display_name`
            // backfills it (and its slug, if drifted) — see
            // `domain::repo::backfill_slug`'s own doc comment for why this
            // has to happen on a listing rather than on a resolve-by-path.
            anyhow::Ok(domain::repo::backfill_slug(backend, cipher, &uid, config, etag).await)
        });
    }
    let mut visible = Vec::new();
    while let Some(result) = set.join_next().await {
        let Ok(Ok(config)) = result else { continue };
        let show = match config.visibility {
            Visibility::Public => true,
            Visibility::Limited => viewer_logged_in,
            Visibility::Private => false,
        };
        if show {
            visible.push(IndexRepo {
                label: crate::webui::repo_label(&config.full_name, config.display_name.as_deref()),
                name: config.full_name,
                description: config.description,
                archived: config.archived,
                topics: config.topics,
                star_count: config.star_count,
                primary_language: config.primary_language,
                last_push: config.last_push,
                last_push_commits: config.last_push_commits,
                last_push_branch: config.last_push_branch,
            });
        }
    }
    Ok(visible)
}

/// Just the `(full_name, label)` pairs out of [`list_visible_repos`] — what
/// `search::web` needs to fan a cross-repo search out over (the name) and to
/// label the hits it gets back, without exposing [`IndexRepo`] (private to
/// this module, and its `archived`/`topics`/`star_count` fields are
/// index-page-only concerns search has no use for).
pub(crate) async fn visible_searchable_repos(
    ctx: &AppContext,
    viewer_logged_in: bool,
) -> Result<Vec<(String, String)>> {
    Ok(list_visible_repos(ctx, viewer_logged_in)
        .await?
        .into_iter()
        .map(|r| (r.name, r.label))
        .collect())
}

/// `true` if the current request can see a repo with this `visibility`:
/// `Public` always; `Limited` needs *a* session, no specific grant on this
/// repo; `Private` needs `is_admin` (sees everything regardless of
/// `access.json`, same reasoning as the admin-page gate) or at least
/// `Permission::Reporter` from `effective_permission` (not `User::repos`
/// directly, so group/team-inherited access counts too) — a `Guest`-tier
/// grant alone does not unlock browsing. Shared with [`super::fork`], which
/// needs the same check on the *source* repo before letting anyone copy it.
pub(crate) async fn can_view_repo(
    ctx: &AppContext,
    headers: &HeaderMap,
    name: &str,
    visibility: Visibility,
) -> Result<bool, AppError> {
    if visibility == Visibility::Public {
        return Ok(true);
    }
    Ok(match login::current_user(ctx, headers).await {
        Some(_) if visibility == Visibility::Limited => true,
        Some(u) if u.is_admin => true,
        Some(u) => {
            gitquarry_core::auth::access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &u, name)
                .await?
                >= Some(Permission::Reporter)
        }
        None => false,
    })
}

pub(crate) fn require_sha(params: &HashMap<String, String>, locale: gitquarry_i18n::Locale) -> Result<&str, AppError> {
    let sha = params
        .get("sha")
        .context(gitquarry_i18n::shared::repo::missing_sha_param(locale))
        .map_err(AppError::bad_request)?;
    validate::validate_sha(sha).map_err(AppError::bad_request)?;
    Ok(sha.as_str())
}

pub(crate) fn require_tag(params: &HashMap<String, String>, locale: gitquarry_i18n::Locale) -> Result<&str, AppError> {
    let tag = params
        .get("tag")
        .context(gitquarry_i18n::shared::repo::missing_tag_param(locale))
        .map_err(AppError::bad_request)?;
    validate::validate_rev(tag).map_err(AppError::bad_request)?;
    Ok(tag.as_str())
}

pub(crate) fn require_number(
    params: &HashMap<String, String>,
    locale: gitquarry_i18n::Locale,
) -> Result<u64, AppError> {
    params
        .get("number")
        .and_then(|s| s.parse().ok())
        .context(gitquarry_i18n::shared::repo::missing_number_param(locale))
        .map_err(AppError::bad_request)
}

pub(crate) fn require_filename(
    params: &HashMap<String, String>,
    locale: gitquarry_i18n::Locale,
) -> Result<&str, AppError> {
    params
        .get("filename")
        .map(String::as_str)
        .context(gitquarry_i18n::shared::repo::missing_filename_param(locale))
        .map_err(AppError::bad_request)
}

pub(crate) async fn handler_repo(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;

    // GitHub-style `.../blob/{branch...}/{path...}` embedded in the same
    // wildcard capture, tried first: a plain repo genuinely named that
    // whole combined string essentially never exists, so this only ever
    // "steals" requests that were never going to resolve as a plain repo
    // name anyway. See `compound_path`'s module doc for why the split can't
    // just assume a fixed segment count.
    if let Some(compound) = compound_path::resolve_compound_path(&ctx, &headers, &raw_repo, &["blob"]).await? {
        let description = compound.config.description.clone();
        let topnav = login::topnav(&ctx, &headers).await;
        return render_repo_view(
            &ctx,
            &headers,
            compound.name,
            compound.resolved,
            compound.config,
            description.as_deref(),
            &topnav,
            compound.mirror,
            "blob",
            &compound.branch,
            &compound.path,
            &params,
            locale,
        )
        .await;
    }

    // Without this, `/..%2f..%2fetc` would be a filesystem path outside
    // mirrors_root and an arbitrary S3 prefix.
    let name = validate::sanitize_path(&raw_repo).map_err(AppError::bad_request)?;

    // Only serve repos that already exist in the bucket: otherwise any
    // request to a made-up path would create an empty bare mirror on disk.
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, &name)
        )));
    };

    let (config, _etag) = resolved.remote.get_repo_config().await?;
    let description = config.description.clone();
    let description = description.as_deref();
    let owner =
        gitquarry_core::auth::access::resolve_owner_display(ctx.backend().as_ref(), &ctx.cipher, config.owner.as_ref())
            .await?;
    let owner = owner.as_deref();

    let view = params.get("view").map_or("about", String::as_str);
    if view == "settings" || view.starts_with("settings_") {
        // Exempt from the visibility check below: it's the only place
        // Private -> Public is reversible from, and (once logged in, if a
        // login method is configured — see the admin gate right below) this
        // is the trusted operator surface, not the anonymous-facing one.
        if let Some(resp) = require_admin(&ctx, &headers, &raw_repo).await {
            return Ok(resp);
        }
        let topnav = login::topnav(&ctx, &headers).await;
        let density = effective_density(&ctx, &headers).await;
        let actions = render_repo_action_buttons(&ctx, &name, &resolved.uid, &headers).await;
        let header = header_bar(
            &name,
            "",
            "settings",
            HeaderExtras {
                description,
                display_name: config.display_name.as_deref(),
                archived: config.archived,
                visibility: config.visibility,
                branches: &[],
                menu_extra: &actions,
                below_header: "",
                sub_tabs: "",
                locale,
            },
        );
        let users = gitquarry_core::auth::access::list_all_users(&ctx.backend(), &ctx.cipher).await?;
        let group_names = gitquarry_core::auth::access::list_group_names(ctx.backend().as_ref()).await?;
        let granted =
            gitquarry_core::auth::access::list_repo_permissions(&ctx.backend(), &ctx.cipher, &resolved.uid).await?;
        let (manifest, _etag) = resolved.remote.get_manifest().await?;
        let mirrors_list = gitquarry_mirror_external::domain::list_with_etags(&ctx, &resolved.uid).await?;
        let mr_links_by_mirror = repo_settings::fetch_mr_links_by_mirror(&ctx, &resolved.uid, &mirrors_list).await?;
        let body = repo_settings::render_settings_section(
            view,
            &name,
            &config,
            owner.unwrap_or(""),
            repo_settings::Namespaces {
                users: &users,
                group_names: &group_names,
            },
            repo_settings::RepoState {
                manifest: &manifest,
                granted: &granted,
                mirrors: &mirrors_list,
                mr_links: &mr_links_by_mirror,
            },
            repo_settings::PageChrome {
                csrf: &crate::webui::csrf_field(&headers),
                message: None,
                pending_mirror_token: None,
                retry: None,
                locale,
            },
        )
        .into_string();
        return Ok(Html(layout(
            &repo_label(&name, config.display_name.as_deref()),
            &header,
            &body,
            &topnav,
            effective_theme(&ctx, &headers).await,
            density,
            locale,
        ))
        .into_response());
    }
    if !can_view_repo(&ctx, &headers, &name, config.visibility).await? {
        // Same response as "doesn't exist": doesn't confirm a private repo
        // is merely hidden vs. genuinely absent.
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, &name)
        )));
    }

    let topnav = login::topnav(&ctx, &headers).await;
    let mirror = ctx.open_mirror_read(&resolved).await?;

    let Some(head) = git::browse::resolve_head(&mirror.path).await? else {
        let size = crate::webui::effective_text_size(&ctx, &headers).await;
        let density = effective_density(&ctx, &headers).await;
        let menu_extra = format!(
            "{} {}",
            render_repo_action_buttons(&ctx, &name, &resolved.uid, &headers).await,
            render_text_size_buttons(&repo_link(&name, "about", "").to_string(), size),
        );
        let header = header_bar(
            &name,
            "",
            "about",
            HeaderExtras {
                description,
                display_name: config.display_name.as_deref(),
                archived: config.archived,
                visibility: config.visibility,
                branches: &[],
                menu_extra: &menu_extra,
                below_header: "",
                sub_tabs: "",
                locale,
            },
        );
        let body = empty_repo_body(&ctx, &name, &headers).await?;
        return Ok(Html(layout(
            &repo_label(&name, config.display_name.as_deref()),
            &header,
            &body,
            &topnav,
            effective_theme(&ctx, &headers).await,
            density,
            locale,
        ))
        .into_response());
    };
    let default_rev = head.strip_prefix("refs/heads/").unwrap_or(&head);

    let rev = params.get("ref").map_or(default_rev, String::as_str);
    let path = params.get("path").map_or("", String::as_str);
    validate::validate_rev(rev).map_err(AppError::bad_request)?;
    validate::validate_tree_path(path).map_err(AppError::bad_request)?;

    render_repo_view(
        &ctx,
        &headers,
        name,
        resolved,
        config,
        description,
        &topnav,
        mirror,
        view,
        rev,
        path,
        &params,
        locale,
    )
    .await
}

/// Reads `filename` from the repo's default branch tree, if any — the
/// pre-fill source for `new_issue`/`new_merge`'s body/description textarea
/// (`ISSUE_TEMPLATE.md`/`MERGE_REQUEST_TEMPLATE.md`, GitHub's own file
/// names and root-level-only location). `None` for a missing file, a repo
/// with no default branch yet, or any git error — this is a convenience,
/// never something worth failing the new-issue/new-MR page render over.
async fn read_repo_template(
    resolved: &gitquarry_core::app::ResolvedRepo,
    mirror_path: &std::path::Path,
    filename: &str,
) -> Option<String> {
    let (manifest, _etag) = resolved.remote.get_manifest().await.ok()?;
    let head = manifest.head?;
    let bytes = git::browse::read_blob(mirror_path, &head, filename).await.ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The shared HTML-rendering tail once a repo, its config, an open mirror,
/// and a view/rev/path are all known — reached either the normal way
/// (`handler_repo`'s `?view=`/`?ref=`/`?path=` dispatch) or via a
/// GitHub-style `.../blob/{branch}/{path}` compound URL, which resolves all
/// of the same inputs by a different route (see `compound_path`).
#[allow(clippy::too_many_arguments)]
async fn render_repo_view(
    ctx: &AppContext,
    headers: &HeaderMap,
    name: String,
    resolved: gitquarry_core::app::ResolvedRepo,
    config: gitquarry_core::storage::entities::RepoConfig,
    description: Option<&str>,
    topnav: &str,
    mirror: gitquarry_core::app::Mirror,
    view: &str,
    rev: &str,
    path: &str,
    params: &HashMap<String, String>,
    locale: gitquarry_i18n::Locale,
) -> Result<Response, AppError> {
    let active_tab = match view {
        "log" | "commit" => "log",
        "refs" | "compare" => "refs",
        "stats" => "stats",
        "about" | "activity" => "about",
        "summary" => "summary",
        "releases" | "release" => "releases",
        "merges" | "merge" | "new_merge" => "merges",
        _ => "tree", // tree, blob, blame
    };
    let density = effective_density(ctx, headers).await;
    let menu_extra = if active_tab == "about" {
        let size = crate::webui::effective_text_size(ctx, headers).await;
        format!(
            "{} {}",
            render_repo_action_buttons(ctx, &name, &resolved.uid, headers).await,
            render_text_size_buttons(&repo_link(&name, "about", rev).to_string(), size),
        )
    } else {
        render_repo_action_buttons(ctx, &name, &resolved.uid, headers).await
    };
    // Fetched once, rendered twice (`below_header` here and `render_about`'s
    // own sidebar copy below) — not gated on density the way it used to be,
    // so a density flip can hide/show whichever copy applies with pure CSS
    // (`.header-lang`/`.sidebar-lang` in base.css) instead of needing a
    // reload to pick the other one up. Still about-only: every other tab
    // never pays for this walk at all.
    let languages = if active_tab == "about" {
        git::browse::language_stats(&mirror.path, rev).await?
    } else {
        Vec::new()
    };
    let below_header = if active_tab == "about" {
        maud::html! { div class="header-lang" { (render_language_bar(&languages, locale)) } }.into_string()
    } else {
        String::new()
    };
    let sub_tabs = String::new();
    // Cheap (`for-each-ref`, no history walk) compared to `language_stats`
    // above, so unlike that one this runs on every tab, not just summary —
    // the ref switcher next to the title is meant to work from wherever
    // you already are, not just the refs tab.
    let branches = git::browse::branch_names(&mirror.path).await?;
    let header = header_bar(
        &name,
        rev,
        active_tab,
        HeaderExtras {
            description,
            display_name: config.display_name.as_deref(),
            archived: config.archived,
            visibility: config.visibility,
            branches: &branches,
            menu_extra: &menu_extra,
            below_header: &below_header,
            sub_tabs: &sub_tabs,
            locale,
        },
    );

    let theme = effective_theme(ctx, headers).await;
    let body = match view {
        "about" => render_about(ctx, &resolved, &mirror.path, &name, rev, &config, headers, &languages).await?,
        "summary" => render_summary(ctx, &mirror.path, &name, rev, headers).await?,
        "activity" => render_activity(ctx, &mirror.path, &name, rev, locale).await?,
        "refs" => render_refs(ctx, &mirror.path, &name, headers, None, None, locale).await?,
        "tree" => {
            render_tree(
                ctx,
                &mirror.path,
                &name,
                config.display_name.as_deref(),
                rev,
                path,
                locale,
            )
            .await?
        }
        "blob" => render_blob(&mirror.path, &name, config.display_name.as_deref(), rev, path, locale).await?,
        "log" => {
            let q = params.get("q").filter(|q| !q.is_empty()).map(String::as_str);
            if let Some(q) = q {
                validate::validate_query(q).map_err(AppError::bad_request)?;
            }
            let skip: usize = params.get("skip").and_then(|s| s.parse().ok()).unwrap_or(0);
            render_log(
                ctx,
                &mirror.path,
                &name,
                config.display_name.as_deref(),
                rev,
                path,
                q,
                skip,
                locale,
            )
            .await?
        }
        "commit" => {
            let sha = require_sha(params, locale)?;
            let (info, diff, truncated) = git::browse::show_commit_capped(&mirror.path, sha, MAX_DIFF_BYTES).await?;
            render_commit(ctx, &mirror.path, &name, &info, &diff, truncated, headers).await?
        }
        "compare" => {
            let base = params
                .get("base")
                .context(gitquarry_i18n::shared::repo::missing_base_param(locale))
                .map_err(AppError::bad_request)?;
            let head = params
                .get("head")
                .context(gitquarry_i18n::shared::repo::missing_head_param(locale))
                .map_err(AppError::bad_request)?;
            validate::validate_rev(base).map_err(AppError::bad_request)?;
            validate::validate_rev(head).map_err(AppError::bad_request)?;
            render_compare(ctx, &mirror.path, &name, base, head, headers, locale).await?
        }
        "stats" => render_stats(&mirror.path, rev, locale).await?,
        "releases" => render_releases_list(ctx, &resolved, &name, headers).await?,
        "release" => {
            let tag = require_tag(params, locale)?;
            render_release_detail(ctx, &resolved, &name, tag, headers, locale).await?
        }
        "merges" => {
            let status = params.get("status").map(String::as_str).unwrap_or("open");
            let q = params.get("q").filter(|q| !q.is_empty()).map(String::as_str);
            if let Some(q) = q {
                validate::validate_query(q).map_err(AppError::bad_request)?;
            }
            render_merges_list(ctx, &resolved, &name, status, q, headers, locale).await?
        }
        "merge" => {
            let number = require_number(params, locale)?;
            render_merge_detail(ctx, &resolved, &name, &mirror.path, number, params, headers, locale).await?
        }
        "new_merge" => {
            let refs = git::list_refs(&mirror.path).await?;
            let ref_names: Vec<&str> = refs.iter().filter_map(|r| r.strip_prefix("refs/heads/")).collect();
            let template = read_repo_template(&resolved, &mirror.path, "MERGE_REQUEST_TEMPLATE.md").await;
            render_new_merge_form(
                &name,
                &resolved.uid,
                &ref_names,
                template.as_deref(),
                NewMergeFormValues {
                    source_repo: "",
                    source_branch: "",
                    target_branch: "",
                    title: "",
                    description: "",
                    error: None,
                },
                headers,
                locale,
            )
            .into_string()
        }
        "blame" => {
            if path.is_empty() {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::repo::blame_requires_path(locale)
                )));
            }
            render_blame(&mirror.path, &name, config.display_name.as_deref(), rev, path, locale).await?
        }
        other => {
            return Err(AppError::bad_request(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::repo::unknown_view(locale, other)
            )));
        }
    };
    // The browser tab reads the repo's public name, same as the heading —
    // the slug is already in the url right next to it.
    let title = repo_label(&name, config.display_name.as_deref());
    Ok(Html(layout(&title, &header, &body, topnav, theme, density, locale)).into_response())
}

/// `git push` never happened yet (see [`git::browse::resolve_head`]) — same
/// clone URL table [`render_clone_urls`] shows on a non-empty repo's
/// summary tab, plus the actual command to push an existing local repo
/// into it. `git push -u origin HEAD` instead of `... origin main`: it
/// pushes whatever branch is currently checked out under its own name and
/// sets it as upstream, so it doesn't need to guess "main" vs "master" vs
/// anything else the pusher happens to be using.
async fn empty_repo_body(ctx: &AppContext, name: &str, headers: &HeaderMap) -> Result<String, AppError> {
    let clone_urls = render_clone_urls(ctx, name, headers).await?;
    Ok(maud::html! {
        p { em { "Empty repo: nothing pushed yet." } }
        (clone_urls)
        h4 { "push an existing repo" }
        pre class="diff" { "git remote add origin <one of the URLs above>\ngit push -u origin HEAD" }
    }
    .into_string())
}
