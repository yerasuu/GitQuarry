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

pub async fn render_commit(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    info: &CommitInfo,
    diff: &str,
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
            @if let Some(username) = &verified_by {
                " " span class="badge ok" { "verified signature (" (username) ")" }
            } @else {
                (signature_badge(signature))
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
        p { (PreEscaped(render_diff_view_buttons(&return_to, diff_view))) }
        @match diff_view {
            DiffView::Unified => div class="scroll-x" { pre class="diff" { (PreEscaped(super::components::color_diff(diff))) } },
            DiffView::Split => div class="scroll-x full-bleed" { (super::components::side_by_side_diff(diff)) },
        }
    }
    .into_string())
}

/// `status` is `CommitInfo::signature_status` (`%G?`), the fallback shown
/// only when [`domain::commit_signature::verified_signer`] found nothing —
/// this differentiates *why* it found nothing: `E` (has a signature, just
/// nothing gitquarry itself can vouch for) still gets a badge — silence
/// there would look identical to `N` (genuinely unsigned) — but a muted
/// one, since it usually just means no registered account's key matches,
/// not that anything is actually wrong. `B`/`R`/`Y` are the one case
/// stronger than "unverified": git's own check (against whatever system
/// trust store this server happens to have, almost always none) came back
/// actively bad.
#[cfg(any(test, feature = "test-util"))]
pub fn signature_badge(status: char) -> Markup {
    html! {
        @match status {
            'G' => { " " span class="badge ok" { "verified signature" } }
            'U' => { " " span class="badge ok" { "valid signature (untrusted key)" } }
            'X' => { " " span class="badge ok" { "valid signature (expired key)" } }
            'B' => { " " span class="badge danger" { "invalid signature" } }
            'R' => { " " span class="badge danger" { "revoked signature" } }
            'Y' => { " " span class="badge danger" { "signature from a key already expired at signing time" } }
            'E' => { " " span class="muted" { "unverified signature (missing key)" } }
            _ => {} // 'N': unsigned, no badge
        }
    }
}
#[cfg(not(any(test, feature = "test-util")))]
fn signature_badge(status: char) -> Markup {
    html! {
        @match status {
            'G' => { " " span class="badge ok" { "verified signature" } }
            'U' => { " " span class="badge ok" { "valid signature (untrusted key)" } }
            'X' => { " " span class="badge ok" { "valid signature (expired key)" } }
            'B' => { " " span class="badge danger" { "invalid signature" } }
            'R' => { " " span class="badge danger" { "revoked signature" } }
            'Y' => { " " span class="badge danger" { "signature from a key already expired at signing time" } }
            'E' => { " " span class="muted" { "unverified signature (missing key)" } }
            _ => {} // 'N': unsigned, no badge
        }
    }
}
