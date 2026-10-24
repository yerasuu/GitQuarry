//! Federation layer 4's live MR-proxy tab on a `/_remote/<peer>/<repo>`
//! view (`docs/federation-concept.md`'s "Capa 4") — list/detail only,
//! always a live call straight to the peer's own public `/_api/pulls*`
//! JSON API, never cached (MR access is per-viewer, caching it would mix
//! viewers' results). The *write* side (`/_remote_pulls/comment`) is
//! `gitquarry-federation`'s job now, proxied from core — see
//! `gitquarry-federation::web::remote_pulls`'s own doc comment on why the
//! read/write split falls exactly there: this half needs no peer-link
//! state at all, just the viewer's own credential secret (fetched via
//! `federation_client::lookup_credential`), so it never needed to move.
//!
//! No diff/commit-list here, unlike the local merge-request view
//! (`crate::webui::repo::merges::render_merge_detail`) — that needs the
//! peer's actual git objects synced locally first (federation layer 2's
//! mirror, a separate concern this tab doesn't take on); this is metadata
//! and comments only.

use crate::webui::{AppError, csrf_field, repo_link};
use anyhow::{Context, Result, bail};
use axum::http::HeaderMap;
use gitquarry_core::auth::entities::User;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::remote_pulls as remote_pulls_t;
use gitquarry_i18n::web::repo::merges as pulls_t;
use gitquarry_wire::{MrDetail, MrStatus, ReviewVerdict};
use maud::{PreEscaped, html};
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the remote-pulls HTTP client")
}

async fn fetch_remote_pull_requests(
    web_base_url: &str,
    repo_name: &str,
    credential_secret: Option<&str>,
) -> Result<Vec<gitquarry_core::storage::merge_requests::MergeRequestMeta>> {
    let client = http_client()?;
    let url = format!("{}/_api/pulls", web_base_url.trim_end_matches('/'));
    let mut req = client.get(&url).query(&[("repo", repo_name)]);
    if let Some(secret) = credential_secret {
        req = req.bearer_auth(secret);
    }
    let resp = req.send().await.with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    resp.json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))
}

async fn fetch_remote_pull_request_detail(
    web_base_url: &str,
    repo_name: &str,
    number: u64,
    credential_secret: Option<&str>,
) -> Result<MrDetail> {
    let client = http_client()?;
    let url = format!("{}/_api/pulls/detail", web_base_url.trim_end_matches('/'));
    let mut req = client
        .get(&url)
        .query(&[("repo", repo_name), ("number", &number.to_string())]);
    if let Some(secret) = credential_secret {
        req = req.bearer_auth(secret);
    }
    let resp = req.send().await.with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    resp.json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))
}

fn status_label(status: MrStatus, locale: Locale) -> String {
    match status {
        MrStatus::Open => pulls_t::open_status(locale),
        MrStatus::Closed => pulls_t::closed_status(locale),
        MrStatus::Merged => pulls_t::merged_status(locale),
    }
}

fn verdict_label(verdict: ReviewVerdict, locale: Locale) -> String {
    match verdict {
        ReviewVerdict::Approved => pulls_t::approved_verdict(locale),
        ReviewVerdict::ChangesRequested => pulls_t::changes_requested_verdict(locale),
        ReviewVerdict::Commented => pulls_t::commented_verdict(locale),
    }
}

/// Takes `peer_web_base_url`/`credential` already resolved by the caller
/// (`webui::remote_repo::handler_remote_repo`) rather than deriving them
/// itself — makes this testable without needing a real linked peer.
pub(super) async fn render_remote_pulls_list(
    peer_web_base_url: &str,
    repo_name: &str,
    link_name: &str,
    credential: Option<&str>,
    locale: Locale,
) -> Result<String, AppError> {
    let prs = fetch_remote_pull_requests(peer_web_base_url, repo_name, credential)
        .await
        .map_err(AppError::bad_request)?;

    Ok(html! {
        div class="table-scroll" {
        table class="list" {
            tr {
                th { (pulls_t::number_column(locale)) }
                th { (pulls_t::title_column(locale)) }
                th { (pulls_t::status_column(locale)) }
                th { (pulls_t::author_column(locale)) }
            }
            @if prs.is_empty() {
                tr { td colspan="4" {
                    div class="empty-state" {
                        span class="empty-state-icon" { (crate::webui::icons::fork()) }
                        div class="empty-state-title" { (remote_pulls_t::no_pulls_or_no_access_hint(locale)) }
                    }
                } }
            }
            @for pr in &prs {
                tr {
                    td { (pr.number) }
                    td { a href=(repo_link(link_name, "merge", "").number(pr.number).to_string()) { (pr.title) } }
                    td { (status_label(pr.status, locale)) }
                    td { (pr.author) }
                }
            }
        }
        }
    }
    .into_string())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_remote_pull_detail(
    peer_web_base_url: &str,
    peer_name: &str,
    repo_name: &str,
    number: u64,
    viewer: Option<&User>,
    credential: Option<&str>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let MrDetail {
        mr: pr,
        comments,
        reviews,
        head_sha,
    } = fetch_remote_pull_request_detail(peer_web_base_url, repo_name, number, credential)
        .await
        .map_err(AppError::bad_request)?;

    Ok(html! {
        h3 { "#" (pr.number) " " (pr.title) }
        p class="muted" {
            (status_label(pr.status, locale)) " — " code { (pr.source_branch) } " -> " code { (pr.target_branch) }
        }
        p class="muted" { (pulls_t::opened_by_prefix(locale)) (pr.author) (pulls_t::opened_by_middle(locale)) (pr.created_at) }
        @if !pr.description.is_empty() {
            p { (pr.description) }
        }

        // Read-only: only commenting is proxied cross-instance
        // (`/_remote_pulls/comment`), so a remote PR's reviews are shown as
        // the peer reports them, with no submit form of their own.
        @let summary = gitquarry_core::domain::merge_request::review_summary(&reviews, head_sha.as_deref());
        h4 { (pulls_t::reviews_heading(locale)) }
        @if !summary.approved_by.is_empty() {
            p class="muted" { (pulls_t::approved_by_prefix(locale)) (summary.approved_by.join(", ")) }
        }
        @if !summary.changes_requested_by.is_empty() {
            p class="muted" { (pulls_t::changes_requested_by_prefix(locale)) (summary.changes_requested_by.join(", ")) }
        }
        @if reviews.is_empty() {
            p class="muted" { (pulls_t::no_reviews_hint(locale)) }
        }
        @for review in &reviews {
            div class="commit-meta" {
                p class="muted" {
                    strong { (review.author) } " " (verdict_label(review.verdict, locale)) " — " (review.created_at)
                    // A peer that predates `MrDetail::head_sha` sends none;
                    // marking every review outdated then would only say the
                    // peer is old.
                    @if head_sha.is_some() && !gitquarry_core::domain::merge_request::review_is_current(review, head_sha.as_deref()) {
                        " — " em { (pulls_t::outdated_review_marker(locale)) }
                        @if let Some(sha) = &review.head_sha {
                            " " code { (gitquarry_core::validate::short_sha(sha)) }
                        }
                    }
                }
                @if !review.body.is_empty() {
                    p { (review.body) }
                }
            }
        }

        h4 { (pulls_t::comments_heading(locale)) }
        @if comments.is_empty() {
            p class="muted" { (pulls_t::no_comments_hint(locale)) }
        }
        @for comment in &comments {
            div class="commit-meta" {
                p class="muted" {
                    strong { (comment.author) } " — " (comment.created_at)
                    // This tab renders no diff at all (see the module doc),
                    // so a line comment can only name its position, never
                    // hang off the line itself the way the local view does.
                    @if let Some(anchor) = &comment.anchor {
                        " — " (pulls_t::line_comment_prefix(locale)) code { (anchor.path) ":" (anchor.line) }
                    }
                }
                p { (comment.body) }
            }
        }
        @if credential.is_some() {
            form method="post" action="/_remote_pulls/comment" {
                (PreEscaped(csrf_field(headers)))
                input type="hidden" name="peer" value=(peer_name);
                input type="hidden" name="repo" value=(repo_name);
                input type="hidden" name="number" value=(pr.number);
                textarea name="body" rows="4" cols="60" {}
                p { button type="submit" { (pulls_t::comment_button(locale)) } }
            }
        } @else if viewer.is_some() {
            p class="muted" {
                (remote_pulls_t::link_credential_prompt_prefix(locale))
                a href="/_account/tokens" { (remote_pulls_t::link_credential_prompt_link(locale)) }
                (remote_pulls_t::link_credential_prompt_suffix(locale))
            }
        } @else {
            p class="muted" { (remote_pulls_t::log_in_to_comment_hint(locale)) }
        }
    }
    .into_string())
}
