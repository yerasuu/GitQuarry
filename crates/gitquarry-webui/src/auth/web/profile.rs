//! `/_profile/<name>`: public list of a user's or a group's repos (`name`
//! is looked up as a username first, then a group name — mutually
//! exclusive, see [`gitquarry_core::auth::access::reserve_namespace`]) —
//! reachable from wherever an owner name shows up (a repo header's
//! "owner:" line, see [`crate::webui::header_bar`]). No login gate, unlike every
//! other `/_*` page: anyone can look. What differs per viewer is which
//! repos show up — `visibility: public` ones always do, `private` ones
//! only when the viewer is logged in as that same user, or (for a group
//! profile) is a member of that group — not admins, not anyone with an
//! explicit grant on one specific repo: this is a profile listing, not the
//! repo's own access check, which still applies independently once you
//! actually open a private repo's page.

use super::login;
use crate::webui::{State_, effective_density, effective_locale, effective_theme, identicon, layout, url_encode};
use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::domain;
use gitquarry_core::storage;
use gitquarry_core::storage::entities::{OwnerRef, RepoConfig, Visibility};
use maud::{Markup, PreEscaped, html};
use std::sync::Arc;
use tokio::task::JoinSet;

pub(crate) async fn handler_profile_get(
    State(ctx): State_,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, crate::webui::AppError> {
    let viewer = login::current_user(&ctx, &headers).await;
    // `name` is either a username or a group name, never both (see
    // `access::reserve_namespace`) — only one of these two point lookups
    // ever finds something, cheap either way compared to the old whole-blob
    // fetch this replaced.
    let group = access::get_group(ctx.backend().as_ref(), &ctx.cipher, &name).await?.0;
    let can_see_private = match &viewer {
        Some(u) if u.username == name => true,
        // Owner or on any team — any relationship to the group is "close
        // enough" to see its private repos listed here, same as membership
        // was before teams existed. The repo's own access check (via
        // `effective_permission`) still applies independently once you
        // actually open one.
        Some(u) => group
            .as_ref()
            .is_some_and(|g| g.owners.contains(&u.uid) || g.teams.iter().any(|t| t.members.contains(&u.uid))),
        None => false,
    };
    // `name` names a user xor a group (see `access::reserve_namespace`) —
    // `group` already answers that; only fetch the `User` when it isn't one.
    let owner_ref = match &group {
        Some(g) => Some(OwnerRef::Group(g.uid.clone())),
        None => access::get_user(ctx.backend().as_ref(), &ctx.cipher, &name)
            .await?
            .0
            .map(|u| OwnerRef::User(u.uid)),
    };
    let description = group.and_then(|g| g.description);
    let repos = list_owned_repos(&ctx, owner_ref.as_ref(), viewer.is_some(), can_see_private).await?;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let is_own_profile = viewer.as_ref().is_some_and(|u| u.username == name);
    // Only a `User` (never a `Group`, which has no `favorites_public`/avatar
    // of its own) can have opted into a public favorites list or uploaded an
    // avatar — see [`super::favorites`] for what the former does with the
    // flag. Already in hand for the viewer's own profile — no need for a
    // second lookup; one shared fetch covers both fields for anyone else's.
    let (favorites_public, avatar_content_type) = match &viewer {
        Some(u) if u.username == name => (u.favorites_public, u.avatar_content_type.clone()),
        _ => match access::get_user(ctx.backend().as_ref(), &ctx.cipher, &name).await?.0 {
            Some(u) => (u.favorites_public, u.avatar_content_type),
            None => (false, None),
        },
    };
    let body = render_profile(
        &name,
        description.as_deref(),
        &repos,
        is_own_profile,
        favorites_public,
        avatar_content_type.as_deref(),
        locale,
    )
    .into_string();
    Ok(Html(layout(
        &name,
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Every repo whose `RepoConfig::owner` uid matches `owner_ref` (`None` if
/// `name` resolved to neither a user nor a group, i.e. nothing to list):
/// `public` always included, `limited` if `viewer_logged_in` (any session,
/// not specifically this profile's), and `private` only if
/// `can_see_private`. Candidate uids come from `list_owned_repo_uids`
/// (`.conf/index/repo-owner/...`), not a bucket-wide scan — a profile only
/// ever needs its own owner's repos, unlike `repo::list_visible_repos`'s
/// global index. The owner-uid compare still runs on the fetched config
/// (not just trusted from the index) since that index is a hint, not a
/// fact: a stale entry from a lost race on a rename/transfer could point at
/// a repo that moved to a different owner since.
async fn list_owned_repos(
    ctx: &AppContext,
    owner_ref: Option<&OwnerRef>,
    viewer_logged_in: bool,
    can_see_private: bool,
) -> Result<Vec<(String, RepoConfig)>> {
    let Some(owner_ref) = owner_ref else {
        return Ok(Vec::new());
    };
    let uids = storage::settings::list_owned_repo_uids(ctx.backend().as_ref(), owner_ref).await?;
    let mut set = JoinSet::new();
    for uid in uids {
        let backend = ctx.backend();
        let cipher = Arc::clone(&ctx.cipher);
        set.spawn(async move {
            // A fetch error hides the repo rather than showing it — same
            // fail-closed choice as the global index.
            let (config, etag) = storage::settings::get_repo_config(backend.as_ref(), uid.as_str(), &cipher).await?;
            // See `domain::repo::backfill_slug`'s own doc comment: this is a
            // uid-based listing, one of the few places that can find and fix
            // a repo predating `display_name`/a drifted slug.
            anyhow::Ok(domain::repo::backfill_slug(backend, cipher, &uid, config, etag).await)
        });
    }
    let mut owned = Vec::new();
    while let Some(result) = set.join_next().await {
        let Ok(Ok(config)) = result else { continue };
        if config.owner.as_ref() != Some(owner_ref) {
            continue;
        }
        let show = match config.visibility {
            Visibility::Public => true,
            Visibility::Limited => viewer_logged_in || can_see_private,
            Visibility::Private => can_see_private,
        };
        if show {
            owned.push((config.full_name.clone(), config));
        }
    }
    owned.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(owned)
}

/// `is_own_profile`: the logged-in viewer's own profile — adds a link into
/// `/_account/*` (the settings sidebar, see `crate::webui::account_settings_page`'s
/// own doc comment) for discoverability, since this public page and that
/// private one are deliberately separate now (a repo listing isn't a
/// "setting"), with nothing else linking one to the other except the topnav
/// dropdown.
///
/// `favorites_public`: whether `username` (a user, never a group — see
/// [`handler_profile_get`]) opted into [`super::favorites`]'s public list;
/// when `true` this links to it so it's actually discoverable, not just
/// reachable by guessing the URL.
pub fn render_profile(
    username: &str,
    description: Option<&str>,
    repos: &[(String, RepoConfig)],
    is_own_profile: bool,
    favorites_public: bool,
    avatar_content_type: Option<&str>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    html! {
        h3 { (identicon::account_avatar(username, avatar_content_type)) (username) }
        @if is_own_profile {
            p class="muted" { a href="/_account/profile" { (gitquarry_i18n::web::auth::profile::account_settings_link(locale)) } }
        }
        @if let Some(description) = description {
            p class="muted" { (description) }
        }
        @if favorites_public {
            p class="muted" {
                a href=(format!("/_profile/{}/favorites", url_encode(username))) { "view favorites" }
            }
        }
        table class="list" {
            tr { th { "repo" } th { "description" } }
            @if repos.is_empty() {
                tr { td colspan="2" { em { (gitquarry_i18n::web::auth::profile::no_repos_visible(locale)) } } }
            }
            @for (name, config) in repos {
                tr {
                    td {
                        a href=(format!("/{}", url_encode(name))) { (crate::webui::repo_label::repo_label(name, config.display_name.as_deref())) }
                        @match config.visibility {
                            Visibility::Public => {}
                            Visibility::Limited => { " " span class="badge" { "limited" } }
                            Visibility::Private => { " " span class="badge" { "private" } }
                        }
                        @if config.archived {
                            " " span class="badge" { "archived" }
                        }
                        @if config.star_count > 0 {
                            " " span class="muted" { (config.star_count) " \u{2605}" }
                        }
                        (PreEscaped(crate::webui::topic_badges(&config.topics)))
                    }
                    td class="muted" { (config.description.as_deref().unwrap_or("")) }
                }
            }
        }
    }
}
