//! `/_repos`: the logged-in viewer's own repo dashboard, split by *how* they
//! reach each repo rather than into one flat list — their own namespace, each
//! group they belong to, and the repos someone granted them individually.
//! `/` stays the public landing page (`Public`/`Limited` only, see
//! [`super::repo::handler_index`]), which is what the "explore" tab points
//! back at.
//!
//! The split is the point: a group's repos are reachable because of a
//! membership that can change at once, an explicit grant is revocable one
//! repo at a time, and a personal repo is neither. A single merged list
//! can't show which is which, so it can't answer "what would I lose if I
//! left this group".

use super::{AppError, State_, effective_density, effective_locale, effective_theme, layout, tab_links, url_encode};
use crate::auth::web::groups::visible_groups;
use crate::auth::web::login;
use crate::auth::web::profile::{list_owned_repos, render_repo_table};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::auth::entities::{Group, User};
use gitquarry_core::storage;
use gitquarry_core::storage::entities::{OwnerRef, RepoConfig};
use maud::{Markup, html};

pub(crate) async fn handler_repos_get(
    State(ctx): State_,
    Query(params): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // Every tab here is "repos *you* reach"; there's nothing to render for
    // a viewer with no session, and `/` already covers what anonymous
    // browsing can see.
    let Some(me) = login::current_user(&ctx, &headers).await else {
        return Ok(Redirect::to(&format!("/_login?return_to={}", url_encode("/_repos"))).into_response());
    };
    let tab = params.get("tab").map(String::as_str).unwrap_or("yours");
    let locale = effective_locale(&ctx, &headers).await;

    let body = match tab {
        "groups" => {
            let groups = visible_groups(&ctx, &me).await?;
            let mut sections = Vec::with_capacity(groups.len());
            for group in groups {
                // `can_see_private: true` — membership in the group (or in
                // one of its ancestors, which is what put it in
                // `visible_groups`) is exactly the relationship
                // `auth::web::group` shows a group's private repos to.
                let owner = OwnerRef::Group(group.uid.clone());
                let repos = list_owned_repos(&ctx, Some(&owner), true, true).await?;
                sections.push((group, repos));
            }
            render_groups_tab(&sections, locale)
        }
        "shared" => render_shared_tab(&granted_repos(&ctx, &me).await?, locale),
        _ => {
            let owner = OwnerRef::User(me.uid.clone());
            render_yours_tab(&list_owned_repos(&ctx, Some(&owner), true, true).await?, locale)
        }
    };

    let page = html! {
        h3 { (gitquarry_i18n::web::repos::repos_title(locale)) }
        (tab_links(None, &tabs(locale), tab))
        // `/_new` is admin-only (repo creation is an operator action on this
        // instance, see `settings::web::require_admin`) — linking it to
        // everyone else would just hand them a 403.
        @if me.is_admin {
            p class="muted" { a href="/_new" { (gitquarry_i18n::web::repos::repos_new_link(locale)) } }
        }
        (body)
    }
    .into_string();
    let topnav = login::topnav(&ctx, &headers).await;
    Ok(Html(layout(
        "repos",
        "",
        &page,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// "explore" is a link back to `/`, not a fourth rendering of the same
/// table: that page already lists everything a viewer can see without a
/// grant, with its own search box and activity sidebar.
fn tabs(locale: gitquarry_i18n::Locale) -> Vec<(&'static str, String, String)> {
    vec![
        (
            "yours",
            "/_repos".to_string(),
            gitquarry_i18n::web::repos::repos_tab_yours(locale),
        ),
        (
            "groups",
            "/_repos?tab=groups".to_string(),
            gitquarry_i18n::web::repos::repos_tab_groups(locale),
        ),
        (
            "shared",
            "/_repos?tab=shared".to_string(),
            gitquarry_i18n::web::repos::repos_tab_shared(locale),
        ),
        (
            "explore",
            "/".to_string(),
            gitquarry_i18n::web::repos::repos_tab_explore(locale),
        ),
    ]
}

/// The repos `me` holds an *explicit* per-repo grant on — `User::repos` keys,
/// resolved to their configs. Deliberately excludes anything reached through
/// a group (that's the groups tab) and anything they own: a grant is the one
/// access path that can be handed out and taken back one repo at a time, so
/// it's worth being able to see exactly which repos depend on one.
///
/// A uid that no longer resolves is skipped, not an error: `User::repos`
/// keeps entries for deleted repos on purpose (uids are never reused, so a
/// stale entry can't re-grant anything).
async fn granted_repos(
    ctx: &gitquarry_core::app::AppContext,
    me: &User,
) -> Result<Vec<(String, RepoConfig)>, AppError> {
    let mut out = Vec::new();
    for uid in me.repos.keys() {
        let Ok((config, _etag)) =
            storage::settings::get_repo_config(ctx.backend().as_ref(), uid.as_str(), &ctx.cipher).await
        else {
            continue;
        };
        if config.deleted_at.is_some() || config.full_name.is_empty() {
            continue;
        }
        // A repo in the viewer's own namespace, or in a group they belong
        // to, already has its own tab — listing it here too would make the
        // split meaningless.
        if config.owner == Some(OwnerRef::User(me.uid.clone())) {
            continue;
        }
        out.push((config.full_name.clone(), config));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub fn render_yours_tab(repos: &[(String, RepoConfig)], locale: gitquarry_i18n::Locale) -> Markup {
    html! {
        h4 { (gitquarry_i18n::web::repos::repos_yours_heading(locale)) }
        @if repos.is_empty() {
            p class="muted" { (gitquarry_i18n::web::repos::repos_none_yours(locale)) }
        } @else {
            (render_repo_table(repos, locale))
        }
    }
}

/// One section per group, each headed by a link to that group's own page —
/// the subgroups a viewer reaches through a parent's membership are separate
/// sections under their own full path, not folded into the parent's, since
/// they're separate namespaces owning separate repos.
pub fn render_groups_tab(sections: &[(Group, Vec<(String, RepoConfig)>)], locale: gitquarry_i18n::Locale) -> Markup {
    html! {
        @if sections.is_empty() {
            p class="muted" { (gitquarry_i18n::web::repos::repos_none_groups(locale)) }
        }
        @for (group, repos) in sections {
            h4 { a href=(format!("/_group/{}", url_encode(&group.name))) { (group.name) } }
            (render_repo_table(repos, locale))
        }
    }
}

pub fn render_shared_tab(repos: &[(String, RepoConfig)], locale: gitquarry_i18n::Locale) -> Markup {
    html! {
        p class="muted" { (gitquarry_i18n::web::repos::repos_shared_prose(locale)) }
        @if repos.is_empty() {
            p class="muted" { (gitquarry_i18n::web::repos::repos_none_shared(locale)) }
        } @else {
            (render_repo_table(repos, locale))
        }
    }
}
