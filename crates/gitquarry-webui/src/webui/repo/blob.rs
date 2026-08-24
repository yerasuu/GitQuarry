//! The `blob` tab: a single file — syntax-highlighted text, an inline
//! image, a "too big"/binary notice, or a link to the raw bytes
//! ([`super::super::export`]).

use super::MAX_BLOB_BYTES;
use crate::webui::{breadcrumbs, export, github_raw_link, highlight, repo_link};
use anyhow::{Context, Result};
use gitquarry_core::git;
use gitquarry_i18n::Locale;
use maud::{PreEscaped, html};
use std::path::Path as FsPath;
use std::time::Duration;

/// How long a blob page waits for its own syntax highlighting before
/// serving the file unstyled instead. Sized against the rest of the page,
/// not against any particular file: whatever is left of a sub-50ms budget
/// once the git reads and the render are paid for.
const HIGHLIGHT_DEADLINE: Duration = Duration::from_millis(30);

pub async fn render_blob(
    bare_repo: &FsPath,
    repo_name: &str,
    display_name: Option<&str>,
    rev: &str,
    path: &str,
    locale: Locale,
) -> Result<String> {
    let breadcrumbs = breadcrumbs(repo_name, display_name, rev, path, "blob", locale);
    let plain_link = github_raw_link(repo_name, rev, path);

    // `blob_id`, not `blob_size`: same one round trip, and the sha it also
    // returns is what keys the highlight cache below.
    let (blob_sha, size) = git::browse::blob_id(bare_repo, rev, path).await?;
    if size > MAX_BLOB_BYTES {
        return Ok(html! {
            (PreEscaped(breadcrumbs))
            p {
                em { "file is " (size) " bytes, too large to display (limit " (MAX_BLOB_BYTES) ")." }
                " " a href=(plain_link) { "view/download raw" } "."
            }
        }
        .into_string());
    }

    if export::is_image(path) {
        return Ok(html! {
            (PreEscaped(breadcrumbs))
            p { img src=(plain_link) alt=(path); }
        }
        .into_string());
    }

    let bytes = git::browse::read_blob(bare_repo, rev, path).await?;
    Ok(match std::str::from_utf8(&bytes) {
        Ok(text) => {
            // syntect highlighting is synchronous CPU work with no async
            // API, same reasoning as `git::browse::archive_one`'s own
            // `spawn_blocking` for gzip/zip repacking — a big file shouldn't
            // stall an async worker thread other requests are scheduled on.
            // Cached by blob sha, so only the first view of a given file
            // version pays it at all.
            let (owned_path, owned_text) = (path.to_string(), text.to_string());
            let mut task = tokio::task::spawn_blocking(move || {
                highlight::highlighted_lines_cached(&blob_sha, &owned_path, &owned_text)
            });
            // Bounded wait, not an unbounded one: syntect costs roughly
            // 0.07ms per line, so a long file would otherwise put the whole
            // page past its budget on the one request that happens to be
            // first. Dropping the handle does not cancel a `spawn_blocking`
            // task, so it runs to completion and fills the cache regardless
            // — this request just renders the file unstyled and the next
            // view of the same blob is a hit.
            let lines = match tokio::time::timeout(HIGHLIGHT_DEADLINE, &mut task).await {
                Ok(done) => done.context("syntax highlighting task panicked")?,
                Err(_) => std::sync::Arc::new(highlight::plain_lines(text)),
            };
            let blame_link = repo_link(repo_name, "blame", rev).path(path).to_string();
            html! {
                div class="crumbs-row" {
                    (PreEscaped(breadcrumbs))
                    p class="blob-actions" {
                        a href=(plain_link) { "view raw" } " · " a href=(blame_link) { "blame" }
                    }
                }
                div class="scroll-x" {
                    table class="blob" {
                        @for (i, line) in lines.iter().enumerate() {
                            @let n = i + 1;
                            tr id=(format!("L{n}")) {
                                td class="lineno" { a href=(format!("#L{n}")) { (n) } }
                                td class="code" { (PreEscaped(line)) }
                            }
                        }
                    }
                }
            }
        }
        Err(_) => {
            html! {
                (PreEscaped(breadcrumbs))
                p { em { "binary file, " (bytes.len()) " bytes." } " " a href=(plain_link) { "download" } "." }
            }
        }
    }
    .into_string())
}
