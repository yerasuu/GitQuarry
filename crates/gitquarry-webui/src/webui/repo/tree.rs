//! The `tree` tab: one directory level (files, subdirs, submodule gitlinks),
//! plus recent commits touching `path` underneath it.

use super::RECENT_LOG_LIMIT;
use super::components::{commit_avatars, commit_table};
use crate::webui::{breadcrumbs, github_blob_link, icons, repo_link};
use anyhow::Result;
use gitquarry_core::app::AppContext;
use gitquarry_core::git;
use gitquarry_core::git::browse::CommitInfo;
use gitquarry_core::validate;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::components as tc;
use gitquarry_i18n::web::repo::tree as t;
use maud::{Markup, PreEscaped, html};
use std::collections::HashMap;
use std::path::Path as FsPath;

pub async fn render_tree(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    display_name: Option<&str>,
    rev: &str,
    path: &str,
    locale: Locale,
) -> Result<String> {
    // Three independent git subprocesses over the same read-only mirror, so
    // they run together rather than one after another: nothing here reads
    // another's output, and awaiting them in sequence just added up their
    // latencies.
    let (entries, gitmodules, recent) = tokio::try_join!(
        git::browse::ls_tree(bare_repo, rev, path),
        git::browse::read_gitmodules(bare_repo, rev),
        git::browse::log(bare_repo, rev, RECENT_LOG_LIMIT, Some(path)),
    )?;
    let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
    // Bounded, single-pass (see `last_touched`'s own doc comment) — one
    // `git log` walk for every row in this listing, not one per row.
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let (avatars, touched) = tokio::join!(
        commit_avatars(ctx, bare_repo, &recent),
        git::browse::last_touched(bare_repo, rev, path, &names),
    );
    let touched = touched?;

    Ok(html! {
        // Only when scoped to a subdirectory: at the repo root, `breadcrumbs`
        // still renders one crumb (the repo name, linking back to `tree`) —
        // redundant here, the repo's already named in the page header right
        // above this tab. Same fix `log::render_log` already got.
        @if !path.is_empty() {
            (PreEscaped(breadcrumbs(repo_name, display_name, rev, path, "tree", locale)))
        }
        div class="table-scroll" {
        table class="list" {
            tr { th { (t::name_column(locale)) } th { (tc::message_column(locale)) } th { (tc::date_column(locale)) } }
            @if !path.is_empty() {
                tr { td colspan="3" { a href=(repo_link(repo_name, "tree", rev).path(parent).to_string()) { ".." } } }
            }
            @for e in &entries {
                (render_entry(repo_name, rev, path, e, &gitmodules, touched.get(&e.name), locale))
            }
        }
        }
        h3 {
            (t::recent_commits_heading(locale))
            @if !path.is_empty() {
                (t::recent_commits_in_path(locale)) (path)
            }
        }
        (commit_table(repo_name, &recent, &avatars, locale))
        p { a href=(repo_link(repo_name, "log", rev).path(path).to_string()) { (t::full_log_link(locale)) } }
    }
    .into_string())
}

fn render_entry(
    repo_name: &str,
    rev: &str,
    path: &str,
    e: &gitquarry_core::git::browse::TreeEntry,
    gitmodules: &HashMap<String, String>,
    last: Option<&CommitInfo>,
    locale: Locale,
) -> Markup {
    let full = if path.is_empty() {
        e.name.clone()
    } else {
        format!("{path}/{}", e.name)
    };
    if e.kind == "commit" {
        // Gitlink: a submodule pointer, not a blob/tree of this repo.
        let submodule_url = gitmodules.get(&full);
        return html! {
            tr {
                td colspan="3" {
                    (icons::fork()) " " (e.name) " @ " (validate::short_sha(&e.sha))
                    " " span class="muted" {
                        @if let Some(url) = submodule_url {
                            (t::submodule_at_url_hint(locale)) (url) ")"
                        } @else {
                            (t::submodule_hint(locale))
                        }
                    }
                }
            }
        };
    }
    let is_dir = e.kind == "tree";
    let href = if is_dir {
        repo_link(repo_name, "tree", rev).path(&full).to_string()
    } else {
        github_blob_link(repo_name, rev, &full)
    };
    html! {
        tr {
            td {
                a href=(href) {
                    @if is_dir { (icons::folder()) } @else { (icons::file()) }
                    " " (e.name)
                }
            }
            td class="muted" {
                @if let Some(c) = last {
                    a href=(repo_link(repo_name, "commit", "").sha(&c.sha).to_string()) { (c.subject) }
                }
            }
            td class="muted" {
                @if let Some(c) = last { (c.date) }
            }
        }
    }
}
