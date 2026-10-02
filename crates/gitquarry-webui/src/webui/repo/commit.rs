//! The `commit` tab: one commit's message, metadata, diffstat, and
//! color-highlighted diff.

use crate::webui::{DiffView, effective_diff_view, raw_link, render_diff_view_buttons, repo_link};
use anyhow::Result;
use axum::http::HeaderMap;
use gitquarry_core::app::AppContext;
use gitquarry_core::domain;
use gitquarry_core::git;
use gitquarry_core::git::browse::CommitInfo;
use gitquarry_core::validate;
use maud::{Markup, PreEscaped, html};
use std::path::Path as FsPath;

/// `truncated` is `git::browse::show_commit_capped`'s verdict: the patch
/// ran past the page's ceiling and what's in `diff` is a prefix. The page
/// says so and points at the raw patch rather than pretending the commit
/// ends there.
#[allow(clippy::too_many_arguments)]
pub async fn render_commit(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    info: &CommitInfo,
    diff: &str,
    truncated: bool,
    headers: &HeaderMap,
) -> Result<String> {
    let stat = git::browse::diffstat(bare_repo, &info.sha).await?;
    let verified_by = domain::commit_signature::verified_signer(
        &ctx.backend(),
        &ctx.cipher,
        bare_repo,
        &info.sha,
        info.signature_status,
        &info.author_email,
    )
    .await?;
    let signature = info.signature_status;
    let diff_view = effective_diff_view(headers);
    let return_to = repo_link(repo_name, "commit", "").sha(&info.sha).to_string();

    Ok(html! {
        h3 {
            "commit " code { (validate::short_sha(&info.sha)) }
            @match &verified_by {
                Some(gitquarry_core::domain::commit_signature::CommitSigner::User(username)) => {
                    " " span class="badge ok" { "verified signature (" (username) ")" }
                }
                // Deliberately not the same badge: this attests that this
                // server built the commit, never that the author stands
                // behind it — only their own key says that.
                // Spelled out here, where there is room for it: the
                // compact row badge only has space for "verified", so this
                // page is what tells a reader *who* verified — and that
                // this instance vouches for having built the commit, not
                // for who wrote the code in it.
                Some(gitquarry_core::domain::commit_signature::CommitSigner::Instance) => {
                    " " span class="badge accent" title="built and signed by this instance" {
                        "verified signature (this instance)"
                    }
                }
                None => (signature_badge(signature)),
            }
        }
        table class="commit-meta" {
            tr { td { "author" } td { (info.author_name) " <" (info.author_email) ">" } }
            tr { td { "date" } td { (info.date) } }
        }
        p { (info.subject) }
        p {
            a href=(raw_link("patch", repo_name, "").sha(&info.sha).to_string()) { "patch" }
            " · "
            a href=(raw_link("rawdiff", repo_name, "").sha(&info.sha).to_string()) { "rawdiff" }
        }
        @if !stat.trim().is_empty() {
            div class="scroll-x" { pre class="diff" { (stat) } }
        }
        @if truncated {
            p class="muted" {
                "this diff is too large to show in full — "
                a href=(raw_link("patch", repo_name, "").sha(&info.sha).to_string()) { "the raw patch" }
                " has all of it"
            }
        }
        p { (PreEscaped(render_diff_view_buttons(&return_to, diff_view))) }
        @match diff_view {
            DiffView::Unified => div class="scroll-x" { pre class="diff" { (PreEscaped(super::components::color_diff(diff))) } },
            DiffView::Split => div class="scroll-x full-bleed" { (super::components::side_by_side_diff(diff)) },
        }
    }
    .into_string())
}

/// `status` is `CommitInfo::signature_status`, the fallback shown only when
/// [`domain::commit_signature::verified_signer`] found nothing. `'U'` (a
/// signature is there, just nothing gitquarry itself can vouch for) still
/// gets a badge — silence would look identical to `'N'`, genuinely unsigned
/// — but a muted one, since it usually just means no registered account's
/// key matches, not that anything is wrong.
///
/// The grade letters this used to render (`G`/`X`/`B`/`R`/`Y`) came from
/// `%G?`, which is git checking against whatever system trust store the
/// server happens to have — almost always none, since this process never
/// populates one. Claiming "valid signature (untrusted key)" off that was
/// asserting more than had been checked, and the walk that produced it cost
/// a `gpg` fork per commit (see `git::browse::CommitInfo::signature_status`),
/// so the letter is now just signed/unsigned and the only real verdict is
/// the one above it.
fn signature_badge(status: char) -> Markup {
    html! {
        @match status {
            'N' => {} // unsigned, no badge
            _ => { " " span class="muted" { "unverified signature (missing key)" } }
        }
    }
}
