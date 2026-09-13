//! Federation layer 2's read-only browse view over a linked peer's synced
//! mirror (`docs/federation-concept.md` capa 2) — [`handler_remote_repo`],
//! reusing [`crate::webui::repo`]'s own `render_tree`/`render_blob`/
//! `render_log`/`render_blame`/`render_commit` verbatim (they only ever
//! needed a bare-repo path) instead of duplicating them.
//!
//! One of the few pieces of the old single-binary federation module that
//! survived the core/federation binary split largely unchanged
//! (`docs/federation-binary-split-concept.md` section 2's table) — only how
//! `{peer}` resolves changed, from a local `.conf/peers` read to
//! `GET /_internal-out/peer/{name}` on the separate `gitquarry-federation`
//! process. Unreachable/unconfigured federation means this whole view is
//! unavailable, same "federation is an optional component" framing every
//! other federation-backed core route already follows.

use super::remote_pulls::{render_remote_pull_detail, render_remote_pulls_list};
use crate::auth::web::login;
use crate::webui;
use crate::webui::federation_client;
use crate::webui::repo::blame::render_blame;
use crate::webui::repo::blob::render_blob;
use crate::webui::repo::commit::render_commit;
use crate::webui::repo::log::render_log;
use crate::webui::repo::tree::render_tree;
use crate::webui::repo::{RECENT_LOG_LIMIT, SUMMARY_REFS_LIMIT, components, require_number, require_sha};
use crate::webui::{AppError, State_, effective_density, effective_locale, effective_theme, layout, repo_link};
use anyhow::Result;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use gitquarry_core::auth::entities::User;
use gitquarry_core::git;
use gitquarry_core::validate;
use gitquarry_i18n::Locale;
use std::collections::HashMap;
use std::path::Path as FsPath;

fn federation_unavailable(locale: Locale) -> AppError {
    AppError::not_found(anyhow::anyhow!(
        "{}",
        gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, "federation")
    ))
}

/// Federation layer 2: browsing one specific repo of a linked peer
/// (`docs/federation-concept.md` capa 2). Deliberately a narrower view set
/// than [`crate::webui::repo::handler_repo`] (`summary`/`tree`/`blob`/`log`/`blame` only, no
/// `settings`/`releases`/`pulls`/`stats`/`compare`/`commit`) — none of those
/// have any meaning without a local `RepoConfig`/uid, and this repo has
/// neither until someone forks it (federation layer 2's other half).
pub(crate) async fn handler_remote_repo(
    State(ctx): State_,
    Path((peer_name, raw_repo)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let repo_name = validate::sanitize_path(&raw_repo).map_err(AppError::bad_request)?;
    let locale = effective_locale(&ctx, &headers).await;

    let Some(federation) = &ctx.federation else {
        return Err(federation_unavailable(locale));
    };
    let Some(peer) = federation_client::lookup_peer(federation, &peer_name).await else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, &peer_name)
        )));
    };
    if peer.banned {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found_in_bucket(locale, &peer_name)
        )));
    }

    let topnav = login::topnav(&ctx, &headers).await;
    let mirror = ctx
        .open_remote_mirror_at(&peer_name, &peer.git_base_url, &repo_name)
        .await
        .map_err(AppError::bad_request)?;
    // Combined so every link `render_tree`/`render_blob`/`render_log`/
    // `render_blame`/their own internal breadcrumbs build (they only ever
    // take a plain `repo_name: &str`) stays under `/_remote/<peer>/<repo>`
    // for free — `repo_link` needed zero changes for this.
    let link_name = format!("_remote/{peer_name}/{repo_name}");

    let Some(head) = git::browse::resolve_head(&mirror.path).await? else {
        let header = render_remote_header(&peer_name, &repo_name, &link_name, "", "summary", locale);
        let body =
            maud::html! { p { em { "Empty repo, or the peer hasn't pushed anything to it yet." } } }.into_string();
        return Ok(Html(layout(
            &repo_name,
            &header,
            &body,
            &topnav,
            effective_theme(&ctx, &headers).await,
            effective_density(&ctx, &headers).await,
            locale,
        ))
        .into_response());
    };
    let default_rev = head.strip_prefix("refs/heads/").unwrap_or(&head);
    let rev = params.get("ref").map_or(default_rev, String::as_str);
    let path = params.get("path").map_or("", String::as_str);
    validate::validate_rev(rev).map_err(AppError::bad_request)?;
    validate::validate_tree_path(path).map_err(AppError::bad_request)?;

    let view = params.get("view").map_or("summary", String::as_str);
    let active_tab = match view {
        "log" | "commit" => "log",
        "summary" => "summary",
        "merges" | "merge" => "merges",
        _ => "tree", // tree, blob, blame
    };
    let theme = effective_theme(&ctx, &headers).await;
    let density = effective_density(&ctx, &headers).await;
    let body = match view {
        "summary" => {
            let viewer = login::current_user(&ctx, &headers).await;
            render_remote_summary(
                &ctx,
                &mirror.path,
                &link_name,
                &peer_name,
                &repo_name,
                rev,
                viewer.as_ref(),
                &headers,
                locale,
            )
            .await?
        }
        "tree" => render_tree(&ctx, &mirror.path, &link_name, None, rev, path, locale).await?,
        "blob" => render_blob(&mirror.path, &link_name, None, rev, path, locale).await?,
        "log" => {
            let q = params.get("q").filter(|q| !q.is_empty()).map(String::as_str);
            if let Some(q) = q {
                validate::validate_query(q).map_err(AppError::bad_request)?;
            }
            let skip: usize = params.get("skip").and_then(|s| s.parse().ok()).unwrap_or(0);
            render_log(&ctx, &mirror.path, &link_name, None, rev, path, q, skip, locale).await?
        }
        // Not one of the three tabs (only reachable via a commit-table row's
        // own link, same as local `handler_repo`'s "commit" isn't a tab
        // either) — supported anyway so `commit_table`'s sha links, reused
        // unmodified, don't 400 with "unknown view".
        "commit" => {
            let sha = require_sha(&params, locale)?;
            let (info, diff) = git::browse::show_commit(&mirror.path, sha).await?;
            render_commit(&ctx, &mirror.path, &link_name, &info, &diff, false, &headers).await?
        }
        // Federation layer 4's live PR-proxy tab — always a fresh call to
        // the peer's own API, never cached like the rest of this view
        // (`open_remote_mirror_at`'s synced git objects are). Resolved here
        // (not inside the render fns) so they take an explicit
        // `viewer`/`credential` like `render_remote_summary` already does.
        "merges" | "merge" => {
            let viewer = login::current_user(&ctx, &headers).await;
            let credential = match &viewer {
                Some(u) => federation_client::lookup_credential(federation, &peer_name, &u.uid).await,
                None => None,
            };
            if view == "merges" {
                render_remote_pulls_list(
                    &peer.web_base_url,
                    &repo_name,
                    &link_name,
                    credential.as_deref(),
                    locale,
                )
                .await?
            } else {
                let number = require_number(&params, locale)?;
                render_remote_pull_detail(
                    &peer.web_base_url,
                    &peer_name,
                    &repo_name,
                    number,
                    viewer.as_ref(),
                    credential.as_deref(),
                    &headers,
                    locale,
                )
                .await?
            }
        }
        "blame" => {
            if path.is_empty() {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::repo::blame_requires_path(locale)
                )));
            }
            render_blame(&mirror.path, &link_name, None, rev, path, locale).await?
        }
        other => {
            return Err(AppError::bad_request(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::repo::unknown_view(locale, other)
            )));
        }
    };
    let header = render_remote_header(&peer_name, &repo_name, &link_name, rev, active_tab, locale);
    Ok(Html(layout(&repo_name, &header, &body, &topnav, theme, density, locale)).into_response())
}

/// Minimal chrome for a remote repo page — deliberately not
/// `page_chrome::header_bar` (its 8-tab bar assumes local-only views like
/// `settings`/`releases`/`pulls`/`stats` that have no meaning here).
fn render_remote_header(
    peer_name: &str,
    repo_name: &str,
    link_name: &str,
    rev: &str,
    active: &str,
    locale: Locale,
) -> String {
    let _ = locale; // reserved: no translated copy needed yet, this header is 3 plain tab labels
    let tabs = [
        ("summary", "summary"),
        ("log", "log"),
        ("tree", "tree"),
        ("merges", "merges"),
    ];
    maud::html! {
        table class="header" {
            tr { td class="main" { a href="/" { "GitQuarry" } " / " (peer_name) " / " (repo_name) " " span class="badge" { "remote" } } }
        }
        table class="tabs" {
            tr { td {
                @for (i, (id, label)) in tabs.iter().enumerate() {
                    @if i > 0 { " | " }
                    a class=[(active == *id).then_some("active")] href=(repo_link(link_name, id, if *id == "summary" || *id == "merges" { "" } else { rev }).to_string()) { (label) }
                }
            } }
        }
    }
    .into_string()
}

/// A remote repo's front page: branches/tags, recent commits, and — logged
/// in only, federation layer 2's other half — a fork-to-my-namespace form.
/// No README/clone-URLs/star-watch buttons (all tied to a local
/// `RepoConfig`/uid this repo doesn't have until forked).
#[allow(clippy::too_many_arguments)]
async fn render_remote_summary(
    ctx: &gitquarry_core::app::AppContext,
    bare_repo: &FsPath,
    link_name: &str,
    peer_name: &str,
    repo_name: &str,
    rev: &str,
    viewer: Option<&User>,
    headers: &HeaderMap,
    locale: Locale,
) -> Result<String, AppError> {
    let refs = git::browse::list_refs_detailed(bare_repo, "refs/").await?;
    let (branches, tags) = components::split_refs(refs);
    let recent = git::browse::log(bare_repo, rev, RECENT_LOG_LIMIT, None).await?;
    let avatars = components::commit_avatars(ctx, bare_repo, &recent).await;
    let branch_avatars =
        components::ref_avatars(ctx, bare_repo, &branches[..branches.len().min(SUMMARY_REFS_LIMIT)]).await;
    let tag_avatars = components::ref_avatars(ctx, bare_repo, &tags[..tags.len().min(SUMMARY_REFS_LIMIT)]).await;
    Ok(maud::html! {
        (components::ref_table(link_name, &branches, SUMMARY_REFS_LIMIT, false, &branch_avatars, locale))
        @if !tags.is_empty() {
            (components::ref_table(link_name, &tags, SUMMARY_REFS_LIMIT, false, &tag_avatars, locale))
        }
        (components::commit_table(link_name, &recent, &avatars, locale))
        @if let Some(viewer) = viewer {
            (render_remote_fork_form(peer_name, repo_name, viewer, headers))
        }
    }
    .into_string())
}

/// Forks straight to the viewer's own namespace — no group picker like
/// local `crate::webui::repo::summary::render_fork_form` offers: keeping this
/// one field-simple since `webui::fork::handler_fork_post` already supports a
/// group destination generically for whoever wants it via a direct form
/// post, this is just the common-case one-click path from the browse page.
fn render_remote_fork_form(peer_name: &str, repo_name: &str, viewer: &User, headers: &HeaderMap) -> maud::Markup {
    let default_name = repo_name.rsplit('/').next().unwrap_or(repo_name);
    maud::html! {
        form method="post" action="/_fork" class="fork-form" {
            (maud::PreEscaped(webui::csrf_field(headers)))
            input type="hidden" name="peer" value=(peer_name);
            input type="hidden" name="source" value=(repo_name);
            input type="hidden" name="namespace" value=(viewer.username);
            input name="name" size="20" value=(default_name) placeholder="repo name";
            button type="submit" class="btn" { (webui::icons::fork()) " fork to " (viewer.username) }
        }
    }
}
