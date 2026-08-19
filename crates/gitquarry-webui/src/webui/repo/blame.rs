//! The `blame` tab: one file, line-by-line, annotated with the commit that
//! last touched each line.

use crate::webui::{breadcrumbs, repo_link};
use anyhow::Result;
use gitquarry_core::git;
use gitquarry_core::validate;
use gitquarry_i18n::Locale;
use maud::{PreEscaped, html};
use std::path::Path as FsPath;

pub async fn render_blame(
    bare_repo: &FsPath,
    repo_name: &str,
    rev: &str,
    path: &str,
    locale: Locale,
) -> Result<String> {
    let lines = git::browse::blame(bare_repo, rev, path).await?;
    Ok(html! {
        (PreEscaped(breadcrumbs(repo_name, rev, path, "blob", locale)))
        div class="scroll-x" {
            table class="blob" {
                @for (i, l) in lines.iter().enumerate() {
                    @let n = i + 1;
                    tr id=(format!("L{n}")) {
                        td class="lineno" {
                            a href=(repo_link(repo_name, "commit", "").sha(&l.sha).to_string()) title=(l.author) {
                                (validate::short_sha(&l.sha))
                            }
                        }
                        td class="code" { (l.content) }
                    }
                }
            }
        }
    }
    .into_string())
}
