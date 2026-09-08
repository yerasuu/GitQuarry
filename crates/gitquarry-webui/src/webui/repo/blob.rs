//! The `blob` tab: a single file — syntax-highlighted text, an inline
//! image, a "too big"/binary notice, or a link to the raw bytes
//! ([`super::super::export`]).
//!
//! Highlighting itself is entirely client-side (`highlight.js`, vendored —
//! see `app.js`'s own doc comment for where it's invoked): this just emits
//! the file's raw text, escaped, inside one `<pre><code id="blob-code"
//! class="language-*">` block. The `id` is what `app.js` looks for to run
//! the line-numbers plugin only on this one full-file block (never on a
//! diff line's own small fragment) — its own doc comment covers why a
//! whole-file block, not one highlighted per line, is what keeps a
//! multi-line comment/string correctly classified, and how `#L42`
//! permalinks (this page's own convention, also what `blame.rs`'s row ids
//! are for) survive that.

use super::MAX_BLOB_BYTES;
use crate::webui::{breadcrumbs, export, github_raw_link, language_tag::language_tag, repo_link};
use anyhow::Result;
use gitquarry_core::git;
use gitquarry_i18n::Locale;
use maud::{PreEscaped, html};
use std::path::Path as FsPath;

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

    let size = git::browse::blob_size(bare_repo, rev, path).await?;
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
            let blame_link = repo_link(repo_name, "blame", rev).path(path).to_string();
            html! {
                div class="crumbs-row" {
                    (PreEscaped(breadcrumbs))
                    p class="blob-actions" {
                        a href=(plain_link) { "view raw" } " · " a href=(blame_link) { "blame" }
                    }
                }
                div class="scroll-x" {
                    pre class="blob" {
                        code id="blob-code" class=(format!("language-{}", language_tag(path))) { (text) }
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
