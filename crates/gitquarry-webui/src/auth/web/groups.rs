//! `/_groups`: manage groups — shared namespaces (`<group>/<repo>`,
//! mutually exclusive with usernames — see
//! [`gitquarry_core::auth::access::reserve_namespace`]) whose [`Team`]s grant members
//! one [`Permission`] tier on either every repo in the group or a specific
//! subset, without a per-repo entry for each one (see
//! `gitquarry_core::auth::access`'s `effective_permission_for`).
//!
//! Unlike [`super::users`] (strictly admin-only), this page is reachable by
//! anyone logged in — what they see is filtered to groups they actually
//! manage: global admins see every group, everyone else sees only groups
//! they're listed as an owner of (or nothing, if none). Creating/deleting a
//! group outright is still admin-only — reserving or freeing a whole
//! namespace is an operator decision — but once a group exists, its owners
//! run it themselves: add/remove other owners, create/delete teams, edit
//! team membership.

use super::login;
use crate::webui::{
    AppError, Flash, State_, admin_header, csrf_field, effective_density, effective_locale, effective_theme, layout,
    non_empty, render_flash, require_csrf, url_encode,
};
use axum::extract::{Form, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{Group, Permission, Team, User};
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::uid::Uid;
use maud::{Markup, PreEscaped, html};
use std::collections::HashMap;

fn login_redirect() -> Response {
    Redirect::to(&format!("/_login?return_to={}", url_encode("/_groups"))).into_response()
}

pub(crate) async fn handler_groups_get(
    State(ctx): State_,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let Some(me) = login::current_user(&ctx, &headers).await else {
        return Ok(login_redirect());
    };
    let mut groups = access::list_all_groups(&ctx.backend(), &ctx.cipher).await?;
    let q = params.get("q").cloned().unwrap_or_default();
    let q_lower = q.to_ascii_lowercase();
    groups.retain(|g| q.is_empty() || g.name.to_ascii_lowercase().contains(&q_lower));
    let names = uid_to_username_map(&ctx.backend(), &ctx.cipher).await?;
    let repo_names = repo_uid_to_name_map(&ctx, &groups).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let body = render_groups_page(
        &me.uid,
        me.is_admin,
        &groups,
        &names,
        &repo_names,
        &q,
        PageChrome {
            csrf: &csrf_field(&headers),
            message: None,
            locale,
        },
    )
    .into_string();
    Ok(axum::response::Html(layout(
        "groups",
        &admin_header("groups", locale),
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Every user's `uid -> username`, for rendering `Group::owners`/
/// `Team::members` (uid-keyed) as clickable names instead of raw hex. Full
/// `list_all_users` fan-out, same cost this page already accepts elsewhere
/// (`can_manage_group_for`'s per-action fetch) — `/_groups` is not a
/// hot/high-traffic path, and this mirrors `repo_settings::render_perm_grid`
/// being handed the whole user list for the same reason.
async fn uid_to_username_map(
    backend: &std::sync::Arc<dyn gitquarry_core::storage::Backend>,
    cipher: &std::sync::Arc<gitquarry_core::crypto::Cipher>,
) -> Result<HashMap<Uid, String>, AppError> {
    let users = access::list_all_users(backend, cipher).await?;
    Ok(users.into_iter().map(|u| (u.uid, u.username)).collect())
}

/// Every distinct repo uid referenced across `groups`' teams' `repos` sets,
/// resolved to its current `full_name` — for rendering `Team::repos`
/// (uid-keyed) as names instead of raw hex. A repo uid that no longer
/// resolves (deleted) is just absent; [`render_team`] falls back to
/// "(repo deleted)" for those.
async fn repo_uid_to_name_map(ctx: &gitquarry_core::app::AppContext, groups: &[Group]) -> HashMap<Uid, String> {
    let uids: std::collections::BTreeSet<Uid> = groups
        .iter()
        .flat_map(|g| g.teams.iter())
        .filter_map(|t| t.repos.as_ref())
        .flat_map(|set| set.iter().cloned())
        .collect();
    let mut set = tokio::task::JoinSet::new();
    for uid in uids {
        let backend = ctx.backend();
        let cipher = std::sync::Arc::clone(&ctx.cipher);
        set.spawn(async move {
            let name = gitquarry_core::storage::settings::get_repo_config(backend.as_ref(), uid.as_str(), &cipher)
                .await
                .ok()
                .map(|(config, _etag)| config.full_name);
            (uid, name)
        });
    }
    let mut map = HashMap::new();
    while let Some(result) = set.join_next().await {
        if let Ok((uid, Some(name))) = result {
            map.insert(uid, name);
        }
    }
    map
}

#[derive(serde::Deserialize)]
pub(crate) struct GroupsForm {
    /// Wire name `group_action`, not `action`: `delete_group`/`delete_team`/
    /// `remove_team_member` are `data-async` -- `HTMLFormElement` is
    /// `[OverrideBuiltins]`, so a control literally named `action` shadows
    /// the form's own `.action` IDL property, and `style/app.js`'s
    /// `fetchFragment(form.action, ...)` got the `<input>` element instead
    /// of the URL, 404-ing. Every form on this page shares this one
    /// struct/field, so the rename applies even to the plain ones too.
    #[serde(rename = "group_action")]
    action: String,
    #[serde(default)]
    csrf: String,
    /// The group's name — every action names one.
    #[serde(default)]
    name: String,
    /// For `add_owner`/`remove_owner`/`add_team_member`/`remove_team_member`.
    #[serde(default)]
    username: String,
    /// A team's name — for every team-scoped action.
    #[serde(default)]
    team: String,
    /// One of the five `Permission` tier names — only read by `add_team`.
    #[serde(default)]
    perm: String,
    /// Comma-separated repo-local names (the part after `<group>/`) —
    /// empty means "every repo in the group". Only read by `add_team`;
    /// there's no separate edit-scope action, delete and recreate the team
    /// instead — keeps this form (and the permission check around it) from
    /// needing its own set of states to get wrong.
    #[serde(default)]
    repos: String,
    /// Read by `add_group` (initial value) and `set_description` (the only
    /// way to change it afterward).
    #[serde(default)]
    description: String,
    /// `mirror::gc::parse_size` syntax (e.g. `"5G"`) — only read by
    /// `set_storage_quota`. Empty clears the limit (unlimited).
    #[serde(default)]
    storage_quota: String,
}

pub(crate) async fn handler_groups_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<GroupsForm>,
) -> Result<Response, AppError> {
    let Some(me) = login::current_user(&ctx, &headers).await else {
        return Ok(login_redirect());
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let audit_detail = match form.action.as_str() {
        "add_owner" | "remove_owner" => Some(form.username.clone()),
        "add_team" | "delete_team" => Some(form.team.clone()),
        "add_team_member" | "remove_team_member" => Some(format!("{}: {}", form.team, form.username)),
        _ => None,
    };

    let action_result = apply_groups_action(&ctx, &me, &form, locale).await?;
    let is_async = headers.contains_key(crate::webui::ASYNC_HEADER);

    // Async is different from the plain fallback below: every form on this
    // page targets `#groups-list` (see `render_groups_list`'s own doc
    // comment), which never contains a flash banner
    // (`render_groups_page`'s own shell has it, above the list) — a
    // page-level fragment swapped into it would corrupt the DOM (`style/
    // app.js`'s `target.replaceWith` taking the whole `<html>` as the
    // replacement). A rejected action goes through `AppError` instead on
    // this path: a real non-2xx status, picked up by
    // `extractFlashMessage`/`showErrorToast` the same way any other failed
    // `data-async` request already is — same reasoning
    // `keys::handler_keys_post`'s own doc comment gives for its `fail!`.
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let invalid_msg;
    let flash = match &action_result {
        Ok(Cas::Ok) => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &me.username,
                &format!("group.{}", form.action),
                &form.name,
                audit_detail,
            )
            .await;
            Flash::Ok("saved to the bucket.")
        }
        Ok(Cas::Conflict) => {
            if is_async {
                return Err(AppError::bad_request(anyhow::anyhow!("{}", cas_conflict_msg)));
            }
            Flash::Err(&cas_conflict_msg)
        }
        Err(msg) => {
            if is_async {
                return Err(AppError::bad_request(anyhow::anyhow!("{msg}")));
            }
            invalid_msg = msg.clone();
            Flash::Err(&invalid_msg)
        }
    };
    let mut groups = access::list_all_groups(&ctx.backend(), &ctx.cipher).await?;
    // A group this same request just created may not show up in the LIST
    // above yet — `create`'s own transaction already returned `Cas::Ok`
    // (the object write landed), but the object-store backend's LIST
    // consistency isn't guaranteed to catch up in the same round trip
    // (varies by provider). Every other action here mutates a group
    // already in `groups`, so this only ever matters for `add_group`:
    // synthesize the row we already know we just wrote instead of trusting
    // the LIST to have it, so the new group never *looks* like creating it
    // silently failed.
    if form.action == "add_group" && !groups.iter().any(|g| g.name == form.name.trim()) {
        groups.push(Group {
            uid: Uid::new(),
            name: form.name.trim().to_string(),
            description: non_empty(form.description.clone()),
            owners: std::collections::BTreeSet::from([me.uid.clone()]),
            teams: Vec::new(),
            storage_quota_bytes: None,
        });
        groups.sort_by(|a, b| a.name.cmp(&b.name));
    }
    let names = uid_to_username_map(&ctx.backend(), &ctx.cipher).await?;
    let repo_names = repo_uid_to_name_map(&ctx, &groups).await;
    // `action_result` is `Ok` past this point (both error arms above
    // already returned) — every successful action refreshes the whole
    // `#groups-list` fragment from a fresh fetch rather than trying to
    // patch just the one card/row that changed.
    if is_async {
        return Ok(axum::response::Html(
            render_groups_list(
                &me.uid,
                me.is_admin,
                &groups,
                &names,
                &repo_names,
                &csrf_field(&headers),
                locale,
            )
            .into_string(),
        )
        .into_response());
    }
    let topnav = login::topnav(&ctx, &headers).await;
    let body = render_groups_page(
        &me.uid,
        me.is_admin,
        &groups,
        &names,
        &repo_names,
        "",
        PageChrome {
            csrf: &csrf_field(&headers),
            message: Some(flash),
            locale,
        },
    )
    .into_string();
    Ok(axum::response::Html(layout(
        "groups",
        &admin_header("groups", locale),
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// One group's worth of read-modify-write, per action — replaces the old
/// single whole-blob CAS: `add_group`/`delete_group` touch only the named
/// group's own object (plus the shared namespace reservation); the other
/// six touch only the one group `form.name` already names.
/// `Ok(Err(msg))` for a rejected action (bad name, unknown group, bad quota
/// string, ...) — the caller folds that into `Flash::Err` and re-renders
/// the groups page instead of losing it to the generic error page.
/// `Err(AppError)` stays reserved for real authorization failures
/// (`forbidden`) and truly exceptional errors (backend/storage via bare
/// `?`) — those two classes are never something a retry on this same page
/// can fix, so they keep the full-page treatment.
async fn apply_groups_action(
    ctx: &gitquarry_core::app::AppContext,
    me: &User,
    form: &GroupsForm,
    locale: gitquarry_i18n::Locale,
) -> Result<Result<Cas, String>, AppError> {
    match form.action.as_str() {
        "add_group" => {
            if !me.is_admin {
                return Err(AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::only_admin_creates_groups(locale)
                )));
            }
            let name = form.name.trim();
            let cas = match domain::group::create(ctx, name, non_empty(form.description.clone()), me.uid.clone()).await
            {
                Ok(cas) => cas,
                Err(e) => return Ok(Err(e.to_string())),
            };
            Ok(cas.ok_or_else(|| gitquarry_i18n::shared::name_already_taken(locale, name)))
        }
        "delete_group" => {
            if !me.is_admin {
                return Err(AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::only_admin_deletes_groups(locale)
                )));
            }
            domain::group::delete(ctx, &form.name).await?;
            Ok(Ok(Cas::Ok))
        }
        // Admin-only, unlike the group-owner-manageable actions below: a
        // group's own owner shouldn't be able to self-raise the storage
        // allowance an admin set for them, the same reason a user can't
        // set their own quota either (`auth::web::users`'s whole route is
        // admin-only already).
        "set_storage_quota" => {
            if !me.is_admin {
                return Err(AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::only_admin_sets_storage_quota(locale)
                )));
            }
            let (group, etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, &form.name).await?;
            let Some(mut group) = group else {
                return Ok(Err(gitquarry_i18n::shared::group_not_found(locale, &form.name)));
            };
            let raw = form.storage_quota.trim();
            let quota_bytes = if raw.is_empty() {
                None
            } else {
                match gitquarry_core::mirror::gc::parse_size(raw) {
                    Ok(bytes) => Some(bytes),
                    Err(e) => return Ok(Err(e.to_string())),
                }
            };
            domain::group::set_storage_quota(&mut group, quota_bytes);
            Ok(Ok(access::put_group(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &group,
                etag.as_deref(),
            )
            .await?))
        }
        "set_description" | "add_owner" | "remove_owner" | "add_team" | "delete_team" | "add_team_member"
        | "remove_team_member" => {
            if !access::can_manage_group_for(ctx.backend().as_ref(), &ctx.cipher, &me.username, &form.name).await? {
                return Err(AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::you_dont_manage_group(locale, &form.name)
                )));
            }
            let (group, etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, &form.name).await?;
            let Some(mut group) = group else {
                return Ok(Err(gitquarry_i18n::shared::group_not_found(locale, &form.name)));
            };
            if let Err(msg) = apply_group_action(ctx, &mut group, form, locale).await {
                return Ok(Err(msg));
            }
            Ok(Ok(access::put_group(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &group,
                etag.as_deref(),
            )
            .await?))
        }
        other => Ok(Err(gitquarry_i18n::shared::unknown_action(locale, other))),
    }
}

/// The five actions [`apply_groups_action`] already confirmed `me` may run
/// on this group (owner or global admin) — split out so that permission
/// check has exactly one call site to get right, not one per action arm.
async fn apply_group_action(
    ctx: &gitquarry_core::app::AppContext,
    group: &mut Group,
    form: &GroupsForm,
    locale: gitquarry_i18n::Locale,
) -> Result<(), String> {
    match form.action.as_str() {
        "set_description" => {
            domain::group::set_description(group, non_empty(form.description.clone()));
        }
        "add_owner" => domain::group::add_owner(ctx, group, &form.username, locale)
            .await
            .map_err(|e| e.to_string())?,
        "remove_owner" => domain::group::remove_owner(ctx, group, &form.username)
            .await
            .map_err(|e| e.to_string())?,
        "add_team" => domain::group::add_team(ctx, group, form.team.trim(), &form.perm, &form.repos, locale)
            .await
            .map_err(|e| e.to_string())?,
        "delete_team" => domain::group::delete_team(group, &form.team),
        "add_team_member" => domain::group::add_team_member(ctx, group, &form.team, &form.username, locale)
            .await
            .map_err(|e| e.to_string())?,
        "remove_team_member" => domain::group::remove_team_member(ctx, group, &form.team, &form.username, locale)
            .await
            .map_err(|e| e.to_string())?,
        _ => unreachable!("caller already matched on these six actions"),
    }
    Ok(())
}

/// `csrf`/`message`/`locale` bundled so [`render_groups_page`] doesn't
/// cross clippy's `too_many_arguments` -- adding `locale` (needed for the
/// "(user deleted)"/"(repo deleted)" placeholders down in
/// [`render_group`]/[`render_team`]) was the 8th positional param.
pub struct PageChrome<'a> {
    pub csrf: &'a str,
    pub message: Option<Flash<'a>>,
    pub locale: gitquarry_i18n::Locale,
}

pub fn render_groups_page(
    viewer: &Uid,
    is_admin: bool,
    groups: &[Group],
    names: &HashMap<Uid, String>,
    repo_names: &HashMap<Uid, String>,
    q: &str,
    chrome: PageChrome<'_>,
) -> Markup {
    let PageChrome { csrf, message, locale } = chrome;
    html! {
        p class="muted" {
            (crate::auth::i18n::groups::groups_prose_stored_in(locale)) code { ".conf/groups/<name>.json" }
            (crate::auth::i18n::groups::groups_prose_inside_bucket(locale)) code { "<group>/<repo>" }
            (crate::auth::i18n::groups::groups_prose_same_shape_as(locale)) code { "<user>/<repo>" }
            (crate::auth::i18n::groups::groups_prose_access_via_teams(locale))
            @for (i, p) in PERMISSION_TIERS.iter().enumerate() {
                @if i > 0 { "/" }
                code { (permission_label(*p, locale)) }
            }
            (crate::auth::i18n::groups::groups_prose_to_members_and_owner(locale))
        }
        (PreEscaped(render_flash(message, locale)))
        form method="get" action="/_groups" {
            input name="q" value=(q) placeholder=(crate::auth::i18n::groups::groups_search_placeholder(locale));
            button type="submit" { (crate::auth::i18n::groups::groups_search_button(locale)) }
        }

        (render_groups_list(viewer, is_admin, groups, names, repo_names, csrf, locale))

        @if is_admin {
            h4 { (crate::auth::i18n::groups::groups_create_heading(locale)) }
            form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                (PreEscaped(csrf))
                input type="hidden" name="group_action" value="add_group";
                input name="name" size="30" placeholder=(crate::auth::i18n::groups::groups_name_placeholder(locale));
                input name="description" size="40" placeholder=(crate::auth::i18n::groups::groups_description_placeholder(locale));
                button type="submit" { (crate::auth::i18n::groups::groups_create_button(locale)) }
            }
        }
    }
}

/// A uid-keyed reference (`Group::owners`/`Team::members` entry) resolved
/// through `names` for display — `None` means the uid no longer resolves
/// (its user was deleted), which gets a plain "(user deleted)" label
/// and no removal form, since there's no username left to resolve back to
/// a uid on submit (see `remove_owner`/`remove_team_member`'s own comment —
/// the orphaned entry is harmless to leave, just not cleanable from here).
fn resolve_name<'a>(names: &'a HashMap<Uid, String>, uid: &Uid) -> Option<&'a str> {
    names.get(uid).map(String::as_str)
}

/// The visible group cards alone, wrapped in a stable `id` — every action
/// on this page (`data-async-target="#groups-list"`, see
/// `crate::webui::ASYNC_HEADER`'s own doc comment) swaps just this fragment
/// instead of re-rendering the whole page.
fn render_groups_list(
    viewer: &Uid,
    is_admin: bool,
    groups: &[Group],
    names: &HashMap<Uid, String>,
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let visible: Vec<&Group> = groups
        .iter()
        .filter(|g| is_admin || g.owners.contains(viewer))
        .collect();
    html! {
        div id="groups-list" {
            @if visible.is_empty() {
                div class="empty-state" { div class="empty-state-title" { (crate::auth::i18n::groups::groups_empty_hint(locale)) } }
            }
            @for group in &visible {
                (render_group(group, is_admin, names, repo_names, csrf, locale))
            }
        }
    }
}

pub fn render_group(
    group: &Group,
    is_admin: bool,
    names: &HashMap<Uid, String>,
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    html! {
        div class="card card-pad user-card" {
            h4 { (group.name) }

            form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                (PreEscaped(csrf))
                input type="hidden" name="group_action" value="set_description";
                input type="hidden" name="name" value=(group.name);
                input name="description" value=(group.description.as_deref().unwrap_or("")) size="60"
                    placeholder=(crate::auth::i18n::groups::groups_description_placeholder(locale));
                button type="submit" { (crate::auth::i18n::groups::groups_save_button(locale)) }
            }

            @if is_admin {
                form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                    (PreEscaped(csrf))
                    input type="hidden" name="group_action" value="set_storage_quota";
                    input type="hidden" name="name" value=(group.name);
                    input name="storage_quota" size="12" placeholder=(crate::auth::i18n::users::storage_quota_placeholder(locale));
                    @if let Some(bytes) = group.storage_quota_bytes {
                        span class="muted" { (crate::auth::i18n::users::storage_quota_current_prefix(locale)) (gitquarry_domain::commands::repack::human_bytes(bytes)) }
                    }
                    button type="submit" { (crate::auth::i18n::users::set_storage_quota_button(locale)) }
                }
            }

            table class="list" {
                tr { th { (gitquarry_i18n::web::chrome::owner_label(locale)) } th {} }
                @if group.owners.is_empty() {
                    tr { td colspan="2" { em { (crate::auth::i18n::groups::groups_no_owners_hint(locale)) } } }
                }
                @for owner in &group.owners {
                    @match resolve_name(names, owner) {
                        Some(username) => (group_action_row(&group.name, "remove_owner", &[("username", username)], &crate::auth::i18n::groups::groups_remove_button(locale), csrf)),
                        None => tr { td { em { (gitquarry_i18n::shared::user_deleted(locale)) } } td {} },
                    }
                }
            }
            form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                (PreEscaped(csrf))
                input type="hidden" name="group_action" value="add_owner";
                input type="hidden" name="name" value=(group.name);
                input name="username" size="30" placeholder=(crate::auth::i18n::groups::groups_username_placeholder(locale));
                button type="submit" { (crate::auth::i18n::groups::groups_add_owner_button(locale)) }
            }

            @for team in &group.teams {
                (render_team(&group.name, team, names, repo_names, csrf, locale))
            }
            h4 style="border-bottom:none" { (crate::auth::i18n::groups::groups_new_team_heading(locale)) }
            form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                (PreEscaped(csrf))
                input type="hidden" name="group_action" value="add_team";
                input type="hidden" name="name" value=(group.name);
                input name="team" size="20" placeholder=(crate::auth::i18n::groups::groups_team_name_placeholder(locale));
                select name="perm" {
                    option value="guest" { (crate::auth::i18n::groups::groups_permission_guest(locale)) }
                    option value="reporter" { (crate::auth::i18n::groups::groups_permission_reporter(locale)) }
                    option value="developer" selected { (crate::auth::i18n::groups::groups_permission_developer(locale)) }
                    option value="maintainer" { (crate::auth::i18n::groups::groups_permission_maintainer(locale)) }
                    option value="owner" { (crate::auth::i18n::groups::groups_permission_owner(locale)) }
                }
                input name="repos" size="30" placeholder=(crate::auth::i18n::groups::groups_repos_scope_placeholder(locale));
                button type="submit" { (crate::auth::i18n::groups::groups_create_team_button(locale)) }
            }

            @if is_admin {
                form method="post" action="/_groups" style="display:inline" data-async data-async-target="#groups-list" {
                    (PreEscaped(csrf))
                    input type="hidden" name="group_action" value="delete_group";
                    input type="hidden" name="name" value=(group.name);
                    button type="submit" { (crate::auth::i18n::groups::groups_delete_group_button(locale)) }
                }
            }
        }
    }
}

/// Every [`Permission`] tier, low to high — shared by the intro prose (lists
/// them all) and the team-add `<select>` (offers them all).
const PERMISSION_TIERS: [Permission; 5] = [
    Permission::Guest,
    Permission::Reporter,
    Permission::Developer,
    Permission::Maintainer,
    Permission::Owner,
];

fn permission_label(perm: Permission, locale: gitquarry_i18n::Locale) -> String {
    match perm {
        Permission::Guest => crate::auth::i18n::groups::groups_permission_guest(locale),
        Permission::Reporter => crate::auth::i18n::groups::groups_permission_reporter(locale),
        Permission::Developer => crate::auth::i18n::groups::groups_permission_developer(locale),
        Permission::Maintainer => crate::auth::i18n::groups::groups_permission_maintainer(locale),
        Permission::Owner => crate::auth::i18n::groups::groups_permission_owner(locale),
    }
}

pub fn render_team(
    group_name: &str,
    team: &Team,
    names: &HashMap<Uid, String>,
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let perm_label = permission_label(team.permission, locale);
    let group_prefix = format!("{group_name}/");
    let scope = match &team.repos {
        None => crate::auth::i18n::groups::all_group_repos_scope(locale),
        Some(repos) => repos
            .iter()
            .map(|uid| match resolve_name(repo_names, uid) {
                // Displayed relative to the group, same as the form field
                // that set it — `strip_prefix` just undoes the
                // `format!("{group_name}/{local}")` `parse_team_repos` did.
                Some(full_name) => full_name.strip_prefix(&group_prefix).unwrap_or(full_name).to_string(),
                None => gitquarry_i18n::shared::repo_deleted(locale),
            })
            .collect::<Vec<_>>()
            .join(", "),
    };
    html! {
        h4 {
            (crate::auth::i18n::groups::groups_team_label_prefix(locale)) (team.name) " "
            span class=(crate::webui::permission_badge_class(team.permission)) { (perm_label) }
            span class="muted" { (crate::auth::i18n::groups::groups_scope_in(locale)) (scope) }
        }
        table class="list" {
            tr { th { (crate::auth::i18n::groups::groups_member_column(locale)) } th {} }
            @if team.members.is_empty() {
                tr { td colspan="2" { em { (crate::auth::i18n::groups::groups_no_members_hint(locale)) } } }
            }
            @for member in &team.members {
                @match resolve_name(names, member) {
                    Some(username) => (group_action_row(group_name, "remove_team_member", &[("team", &team.name), ("username", username)], &crate::auth::i18n::groups::groups_remove_button(locale), csrf)),
                    None => tr { td { em { (gitquarry_i18n::shared::user_deleted(locale)) } } td {} },
                }
            }
        }
        form method="post" action="/_groups" data-async data-async-target="#groups-list" {
            (PreEscaped(csrf))
            input type="hidden" name="group_action" value="add_team_member";
            input type="hidden" name="name" value=(group_name);
            input type="hidden" name="team" value=(team.name);
            input name="username" size="30" placeholder=(crate::auth::i18n::groups::groups_username_placeholder(locale));
            button type="submit" { (crate::auth::i18n::groups::groups_add_member_button(locale)) }
        }
        form method="post" action="/_groups" style="display:inline" data-async data-async-target="#groups-list" {
            (PreEscaped(csrf))
            input type="hidden" name="group_action" value="delete_team";
            input type="hidden" name="name" value=(group_name);
            input type="hidden" name="team" value=(team.name);
            button type="submit" { (crate::auth::i18n::groups::groups_delete_team_button(locale)) }
        }
    }
}

/// One `<tr>` with a label and a same-row `action`-form button — the shape
/// every owner/team-member removal row shares, just with a different
/// action and a different set of hidden fields to carry. Every action on
/// this page now targets `#groups-list` (see `render_groups_list`'s own
/// doc comment), so there's nothing left to make optional here.
fn group_action_row(group_name: &str, action: &str, hidden: &[(&str, &str)], button_label: &str, csrf: &str) -> Markup {
    let label = hidden.last().map(|(_, v)| *v).unwrap_or_default();
    html! {
        tr {
            td { (label) }
            td {
                form method="post" action="/_groups" style="display:inline" data-async data-async-target="#groups-list" {
                    (PreEscaped(csrf))
                    input type="hidden" name="group_action" value=(action);
                    input type="hidden" name="name" value=(group_name);
                    @for (field, value) in hidden {
                        input type="hidden" name=(field) value=(value);
                    }
                    button type="submit" { (button_label) }
                }
            }
        }
    }
}
