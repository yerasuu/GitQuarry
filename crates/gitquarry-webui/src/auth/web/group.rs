//! `/_group/<path>`: one group's own page — its repos, its subgroups, and
//! (to its own members) who belongs to it. The read side of what
//! [`super::groups`] manages, and the group counterpart of
//! [`super::profile`]: a group's `RepoConfig::owner` link lands on
//! `/_profile/<name>`, which redirects here rather than rendering a second,
//! poorer version of this page.
//!
//! `<path>` is a full group path (`acme/team` for a subgroup), so the route
//! is a catch-all. Nothing here is login-gated — anyone may look — but what
//! shows up differs per viewer: `Public` repos always, `Limited` ones to any
//! session, `Private` ones and the member list only to the group's own
//! members (at any tier, inherited from a parent included) and to global
//! admins.

use super::login;
use super::profile::{list_owned_repos, render_repo_table};
use crate::webui::{
    AppError, State_, effective_density, effective_locale, effective_theme, identicon, layout, url_encode,
};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Group, Permission};
use gitquarry_core::storage::entities::{OwnerRef, RepoConfig};
use gitquarry_core::storage::uid::Uid;
use maud::{Markup, html};
use std::collections::HashMap;

pub(crate) async fn handler_group_get(
    State(ctx): State_,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // The whole chain, not just this group: a member of `acme` is a member
    // here too (`access::effective_permission` resolves the same way), so
    // the visibility decision below has to see the ancestors.
    let chain = access::ancestor_group_chain(ctx.backend().as_ref(), &ctx.cipher, &path).await?;
    let Some(group) = chain.last().filter(|g| g.name == path).cloned() else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::group_not_found(effective_locale(&ctx, &headers).await, &path)
        )));
    };
    let viewer = login::current_user(&ctx, &headers).await;
    let is_member = viewer
        .as_ref()
        .is_some_and(|u| u.is_admin || chain.iter().any(|g| g.role_of(&u.uid).is_some()));
    let can_manage = access::can_manage_group(viewer.as_ref(), &chain);

    let owner_ref = OwnerRef::Group(group.uid.clone());
    let repos = list_owned_repos(&ctx, Some(&owner_ref), viewer.is_some(), is_member).await?;
    let subgroups = access::list_child_groups(ctx.backend().as_ref(), &ctx.cipher, &group.uid).await?;
    // Only the group's own members get a member list at all, so there's no
    // point resolving usernames for anyone else.
    let names = if is_member {
        uid_to_username_map(&ctx).await?
    } else {
        HashMap::new()
    };

    let locale = effective_locale(&ctx, &headers).await;
    let body = render_group_page(
        &group, &chain, &repos, &subgroups, &names, is_member, can_manage, locale,
    )
    .into_string();
    let topnav = login::topnav(&ctx, &headers).await;
    Ok(Html(layout(
        &group.name,
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Same whole-user fan-out `super::groups` accepts for the same reason: a
/// group page renders a bounded member list, and there's no uid → username
/// index to read instead.
async fn uid_to_username_map(ctx: &gitquarry_core::app::AppContext) -> Result<HashMap<Uid, String>, AppError> {
    let users = access::list_all_users(&ctx.backend(), &ctx.cipher).await?;
    Ok(users.into_iter().map(|u| (u.uid, u.username)).collect())
}

/// `chain` is `group`'s own ancestor chain, outermost first, with `group`
/// itself last — it drives both the breadcrumb and the "(inherited)" marker
/// on a member whose tier comes from a parent rather than from this group's
/// own `members` map.
#[allow(clippy::too_many_arguments)]
pub fn render_group_page(
    group: &Group,
    chain: &[Group],
    repos: &[(String, RepoConfig)],
    subgroups: &[Group],
    names: &HashMap<Uid, String>,
    is_member: bool,
    can_manage: bool,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    html! {
        h3 {
            (identicon::account_avatar(&group.name, None))
            // Each ancestor is its own link: a subgroup page is the only
            // place the group above it is reachable from.
            @for (i, ancestor) in chain.iter().enumerate() {
                @if i > 0 { " / " }
                @if ancestor.name == group.name {
                    (last_segment(&ancestor.name))
                } @else {
                    a href=(format!("/_group/{}", url_encode(&ancestor.name))) { (last_segment(&ancestor.name)) }
                }
            }
        }
        @if let Some(description) = &group.description {
            p class="muted" { (description) }
        }
        @if can_manage {
            p class="muted" { a href="/_groups" { (gitquarry_i18n::web::auth::group::group_manage_link(locale)) } }
        }

        h4 { (gitquarry_i18n::web::auth::group::group_repos_heading(locale)) }
        (render_repo_table(repos, locale))

        h4 { (gitquarry_i18n::web::auth::group::group_subgroups_heading(locale)) }
        table class="list" {
            tr { th { "group" } th { "description" } }
            @if subgroups.is_empty() {
                tr { td colspan="2" { em { (gitquarry_i18n::web::auth::group::group_no_subgroups(locale)) } } }
            }
            @for sub in subgroups {
                tr {
                    td { a href=(format!("/_group/{}", url_encode(&sub.name))) { (last_segment(&sub.name)) } }
                    td class="muted" { (sub.description.as_deref().unwrap_or("")) }
                }
            }
        }

        h4 { (gitquarry_i18n::web::auth::groups::groups_members_heading(locale)) }
        @if !is_member {
            p class="muted" { (gitquarry_i18n::web::auth::group::group_members_hidden(locale)) }
        } @else {
            table class="list" {
                tr {
                    th { (gitquarry_i18n::web::auth::groups::groups_member_column(locale)) }
                    th { (gitquarry_i18n::web::auth::groups::groups_role_column(locale)) }
                }
                @let members = effective_chain_members(chain);
                @if members.is_empty() {
                    tr { td colspan="2" { em { (gitquarry_i18n::web::auth::groups::groups_no_members_hint(locale)) } } }
                }
                @for (uid, (perm, inherited)) in &members {
                    tr {
                        td {
                            @match names.get(uid) {
                                Some(username) => a href=(format!("/_profile/{}", url_encode(username))) { (username) },
                                None => em { (gitquarry_i18n::shared::auth::user_deleted(locale)) },
                            }
                        }
                        td {
                            span class=(crate::webui::permission_badge_class(*perm)) {
                                (crate::webui::permission_label(*perm, locale))
                            }
                            @if *inherited {
                                span class="muted" { (gitquarry_i18n::web::auth::group::group_inherited_role_suffix(locale)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The group path's last segment — what a breadcrumb link shows, since the
/// segments before it are already their own links to the left of it.
fn last_segment(name: &str) -> &str {
    name.rsplit_once('/').map_or(name, |(_, last)| last)
}

/// Everyone with a tier anywhere in `chain`, at the highest tier they reach,
/// plus whether that tier came from an ancestor rather than from the
/// innermost group itself. Inheritance is what makes a subgroup useful, so
/// its page has to show the members it inherited — but marking them keeps
/// the page from reading as though this group granted the access.
fn effective_chain_members(chain: &[Group]) -> std::collections::BTreeMap<Uid, (Permission, bool)> {
    let mut out = std::collections::BTreeMap::new();
    let own = chain.last().map(Group::effective_members).unwrap_or_default();
    for group in chain {
        for (uid, perm) in group.effective_members() {
            let inherited = own.get(&uid).is_none_or(|own_perm| *own_perm < perm);
            let slot = out.entry(uid).or_insert((perm, inherited));
            if slot.0 < perm {
                *slot = (perm, inherited);
            }
        }
    }
    out
}
