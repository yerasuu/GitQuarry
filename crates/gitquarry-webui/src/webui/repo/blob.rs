//! The `blob` tab: a single file — syntax-highlighted text, an inline
//! image, a "too big"/binary notice, or a link to the raw bytes
//! ([`super::super::export`]).

use super::MAX_BLOB_BYTES;
use crate::webui::{breadcrumbs, export, github_raw_link, highlight, repo_link};
use anyhow::Result;
use gitquarry_core::git;
use gitquarry_i18n::Locale;
use maud::{PreEscaped, html};
use std::path::Path as FsPath;

pub async fn render_blob(bare_repo: &FsPath, repo_name: &str, rev: &str, path: &str, locale: Locale) -> Result<String> {
    let breadcrumbs = breadcrumbs(repo_name, rev, path, "blob", locale);
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
            let lines = highlight::highlighted_lines(path, text);
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
