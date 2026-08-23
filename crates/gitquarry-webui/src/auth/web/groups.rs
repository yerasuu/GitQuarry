//! `/_groups`: manage groups — shared namespaces (`<group>/<repo>`,
//! mutually exclusive with usernames — see
//! [`gitquarry_core::auth::access::reserve_namespace`]) whose members each
//! hold one [`Permission`] tier across every repo in the group, GitLab-
//! shaped: no per-repo entry, no intermediate team object (see
//! `gitquarry_core::auth::access`'s `effective_permission_for`).
//!
//! Unlike [`super::users`] (strictly admin-only), this page is reachable by
//! anyone logged in — what they see is filtered to groups they actually
//! belong to: global admins see every group, everyone else sees only the
//! groups they're a member of (or nothing, if none). Editing a group's
//! members needs the `Owner` tier in it (`access::can_manage_group`), so a
//! `Developer` member sees their group listed read-only. Creating/deleting
//! a group outright stays admin-only — reserving or freeing a whole
//! namespace is an operator decision — but a *subgroup* only extends a path
//! its parent already owns, so anyone who may manage that parent may create
//! one under it.

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
use gitquarry_core::auth::entities::{Group, Permission, User};
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
    let mut groups = visible_groups(&ctx, &me).await?;
    let q = params.get("q").cloned().unwrap_or_default();
    let q_lower = q.to_ascii_lowercase();
    groups.retain(|g| q.is_empty() || g.name.to_ascii_lowercase().contains(&q_lower));
    let names = uid_to_username_map(&ctx.backend(), &ctx.cipher).await?;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let body = render_groups_page(
        &me.uid,
        me.is_admin,
        &groups,
        &names,
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

/// The groups this page may show `me`: every group in the bucket for a
/// global admin, and only the ones they actually belong to otherwise —
/// read from `access::list_groups_for_user`, not by fetching every group and
/// testing its member map, which is the whole-bucket scan the index exists
/// to avoid. Subgroups of a group `me` belongs to are included: membership
/// is inherited downward (`access::effective_permission`), so a group whose
/// repos `me` can reach has to be reachable from this page too.
pub(crate) async fn visible_groups(
    ctx: &gitquarry_core::app::AppContext,
    me: &User,
) -> anyhow::Result<Vec<gitquarry_core::auth::entities::Group>> {
    if me.is_admin {
        return access::list_all_groups(&ctx.backend(), &ctx.cipher).await;
    }
    let direct = access::list_groups_for_user(ctx.backend().as_ref(), &ctx.cipher, &me.uid).await?;
    let mut groups = Vec::new();
    let mut frontier = direct;
    // Breadth-first over the child index, bounded by `MAX_GROUP_DEPTH`
    // levels the same way `ancestor_group_names` bounds the other
    // direction — a subtree can be wide, but never deeper than a group
    // path is allowed to be.
    for _ in 0..access::MAX_GROUP_DEPTH {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for group in frontier {
            next.extend(access::list_child_groups(ctx.backend().as_ref(), &ctx.cipher, &group.uid).await?);
            groups.push(group);
        }
        frontier = next;
    }
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(groups)
}

/// Every user's `uid -> username`, for rendering [`Group::members`]
/// (uid-keyed) as clickable names instead of raw hex. Full `list_all_users`
/// fan-out, same cost this page already accepts elsewhere
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

#[derive(serde::Deserialize)]
pub(crate) struct GroupsForm {
    /// Wire name `group_action`, not `action`: `delete_group`/
    /// `remove_member` are `data-async` -- `HTMLFormElement` is
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
    /// For `set_member`/`remove_member`.
    #[serde(default)]
    username: String,
    /// One of the five `Permission` tier names — only read by `set_member`,
    /// which both adds a new member and changes an existing one's tier.
    #[serde(default)]
    perm: String,
    /// Read by `add_group` (initial value) and `set_description` (the only
    /// way to change it afterward).
    #[serde(default)]
    description: String,
    /// Only read by `add_group`: the full path of an existing group to nest
    /// the new one under, or empty for a top-level group. A `<select>` in the
    /// rendered form, never trusted as-is — `apply_groups_action` re-checks
    /// that the submitter may manage whatever it names.
    #[serde(default)]
    parent: String,
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
        "set_member" => Some(format!("{}: {}", form.username, form.perm)),
        "remove_member" => Some(form.username.clone()),
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
    let mut groups = visible_groups(&ctx, &me).await?;
    // A group this same request just created may not show up in the LIST
    // above yet — `create`'s own transaction already returned `Cas::Ok`
    // (the object write landed), but the object-store backend's LIST
    // consistency isn't guaranteed to catch up in the same round trip
    // (varies by provider). Every other action here mutates a group
    // already in `groups`, so this only ever matters for `add_group`:
    // synthesize the row we already know we just wrote instead of trusting
    // the LIST to have it, so the new group never *looks* like creating it
    // silently failed.
    if form.action == "add_group"
        && !groups
            .iter()
            .any(|g| g.name == full_group_path(&form.parent, &form.name))
    {
        groups.push(Group {
            uid: Uid::new(),
            name: full_group_path(&form.parent, &form.name),
            description: non_empty(form.description.clone()),
            members: std::collections::BTreeMap::from([(me.uid.clone(), Permission::Owner)]),
            parent: None,
            storage_quota_bytes: None,
            legacy_owners: std::collections::BTreeSet::new(),
            legacy_teams: Vec::new(),
        });
        groups.sort_by(|a, b| a.name.cmp(&b.name));
    }
    let names = uid_to_username_map(&ctx.backend(), &ctx.cipher).await?;
    // `action_result` is `Ok` past this point (both error arms above
    // already returned) — every successful action refreshes the whole
    // `#groups-list` fragment from a fresh fetch rather than trying to
    // patch just the one card/row that changed.
    if is_async {
        return Ok(axum::response::Html(
            render_groups_list(&me.uid, me.is_admin, &groups, &names, &csrf_field(&headers), locale).into_string(),
        )
        .into_response());
    }
    let topnav = login::topnav(&ctx, &headers).await;
    let body = render_groups_page(
        &me.uid,
        me.is_admin,
        &groups,
        &names,
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
/// group's own object (plus the shared namespace reservation); the rest
/// touch only the one group `form.name` already names.
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
            // A top-level group reserves a brand-new namespace, which stays
            // an operator decision; a subgroup only extends a path its
            // parent already holds, so managing that parent is enough.
            let parent = match non_empty(form.parent.trim().to_string()) {
                Some(parent_name) => {
                    if !access::can_manage_group_for(ctx.backend().as_ref(), &ctx.cipher, &me.username, &parent_name)
                        .await?
                    {
                        return Err(AppError::forbidden(anyhow::anyhow!(
                            "{}",
                            gitquarry_i18n::shared::auth::you_dont_manage_group(locale, &parent_name)
                        )));
                    }
                    match access::get_group(ctx.backend().as_ref(), &ctx.cipher, &parent_name)
                        .await?
                        .0
                    {
                        Some(parent) => Some(parent),
                        None => {
                            return Ok(Err(gitquarry_i18n::shared::auth::group_not_found(locale, &parent_name)));
                        }
                    }
                }
                None => {
                    if !me.is_admin {
                        return Err(AppError::forbidden(anyhow::anyhow!(
                            "{}",
                            gitquarry_i18n::shared::auth::only_admin_creates_groups(locale)
                        )));
                    }
                    None
                }
            };
            let segment = form.name.trim();
            let cas = match domain::group::create(
                ctx,
                segment,
                non_empty(form.description.clone()),
                me.uid.clone(),
                parent.as_ref(),
                locale,
            )
            .await
            {
                Ok(cas) => cas,
                Err(e) => return Ok(Err(e.to_string())),
            };
            let path = full_group_path(&form.parent, &form.name);
            Ok(cas.ok_or_else(|| gitquarry_i18n::shared::auth::name_already_taken(locale, &path)))
        }
        "delete_group" => {
            if !me.is_admin {
                return Err(AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::only_admin_deletes_groups(locale)
                )));
            }
            if let Err(e) = domain::group::delete(ctx, &form.name, locale).await {
                return Ok(Err(e.to_string()));
            }
            Ok(Ok(Cas::Ok))
        }
        // Admin-only, unlike the owner-manageable actions below: a group's
        // own owner shouldn't be able to self-raise the storage allowance
        // an admin set for them, the same reason a user can't set their own
        // quota either (`auth::web::users`'s whole route is admin-only
        // already).
        "set_storage_quota" => {
            if !me.is_admin {
                return Err(AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::only_admin_sets_storage_quota(locale)
                )));
            }
            let (group, etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, &form.name).await?;
            let Some(mut group) = group else {
                return Ok(Err(gitquarry_i18n::shared::auth::group_not_found(locale, &form.name)));
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
        "set_description" | "set_member" | "remove_member" => {
            if !access::can_manage_group_for(ctx.backend().as_ref(), &ctx.cipher, &me.username, &form.name).await? {
                return Err(AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::you_dont_manage_group(locale, &form.name)
                )));
            }
            let (group, etag) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, &form.name).await?;
            let Some(mut group) = group else {
                return Ok(Err(gitquarry_i18n::shared::auth::group_not_found(locale, &form.name)));
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

/// The three actions [`apply_groups_action`] already confirmed `me` may run
/// on this group (`Owner` member or global admin) — split out so that
/// permission check has exactly one call site to get right, not one per
/// action arm.
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
        "set_member" => domain::group::set_member(ctx, group, &form.username, &form.perm, locale)
            .await
            .map_err(|e| e.to_string())?,
        "remove_member" => domain::group::remove_member(ctx, group, &form.username)
            .await
            .map_err(|e| e.to_string())?,
        _ => unreachable!("caller already matched on these three actions"),
    }
    Ok(())
}

/// `csrf`/`message`/`locale` bundled so [`render_groups_page`] doesn't
/// cross clippy's `too_many_arguments` -- adding `locale` (needed for the
/// "(user deleted)" placeholder down in [`render_group`]) was the 8th
/// positional param.
pub struct PageChrome<'a> {
    pub csrf: &'a str,
    // `pub` only for cross-crate `tests/*.rs` under `test-util`; `Flash`
    // goes `pub(crate)` outside that config, which is what makes plain
    // `cargo check` flag this field as exposing a private type.
    #[cfg_attr(not(any(test, feature = "test-util")), allow(private_interfaces))]
    pub message: Option<Flash<'a>>,
    pub locale: gitquarry_i18n::Locale,
}

pub fn render_groups_page(
    viewer: &Uid,
    is_admin: bool,
    groups: &[Group],
    names: &HashMap<Uid, String>,
    q: &str,
    chrome: PageChrome<'_>,
) -> Markup {
    let PageChrome { csrf, message, locale } = chrome;
    // Every group this viewer may nest a subgroup under. Full paths, so the
    // `<option>` value is exactly what `full_group_path` prepends.
    let manageable = manageable_group_names(groups, viewer, is_admin);
    html! {
        p class="muted" {
            (gitquarry_i18n::web::auth::groups::groups_prose_stored_in(locale)) code { ".conf/groups/<name>.json" }
            (gitquarry_i18n::web::auth::groups::groups_prose_inside_bucket(locale)) code { "<group>/<repo>" }
            (gitquarry_i18n::web::auth::groups::groups_prose_same_shape_as(locale)) code { "<user>/<repo>" }
            (gitquarry_i18n::web::auth::groups::groups_prose_access_via_members(locale))
            @for (i, p) in PERMISSION_TIERS.iter().enumerate() {
                @if i > 0 { "/" }
                code { (crate::webui::permission_label(*p, locale)) }
            }
            (gitquarry_i18n::web::auth::groups::groups_prose_role_applies_to_every_repo(locale))
        }
        (PreEscaped(render_flash(message, locale)))
        form method="get" action="/_groups" {
            input name="q" value=(q) placeholder=(gitquarry_i18n::web::auth::groups::groups_search_placeholder(locale));
            button type="submit" { (gitquarry_i18n::web::auth::groups::groups_search_button(locale)) }
        }

        (render_groups_list(viewer, is_admin, groups, names, csrf, locale))

        // A non-admin still gets this form as long as they own at least one
        // group to nest under — the `<select>` below is what decides whether
        // there's anywhere they may create.
        @if is_admin || !manageable.is_empty() {
            h4 { (gitquarry_i18n::web::auth::groups::groups_create_heading(locale)) }
            form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                (PreEscaped(csrf))
                input type="hidden" name="group_action" value="add_group";
                select name="parent" {
                    @if is_admin {
                        option value="" { (gitquarry_i18n::web::auth::groups::groups_parent_none(locale)) }
                    }
                    @for name in &manageable {
                        option value=(name) { (name) "/" }
                    }
                }
                input name="name" size="30" placeholder=(gitquarry_i18n::web::auth::groups::groups_name_placeholder(locale));
                input name="description" size="40" placeholder=(gitquarry_i18n::web::auth::groups::groups_description_placeholder(locale));
                button type="submit" { (gitquarry_i18n::web::auth::groups::groups_create_button(locale)) }
            }
        }
    }
}

/// Every group in `groups` the viewer may create inside — a new subgroup, a
/// new repo, a fork's destination. That is `Permission::Owner` in the group
/// itself *or* in any group it is nested under, since
/// `access::can_manage_group` (which the POST handlers actually enforce)
/// walks the same chain: an `Owner` of `acme` runs `acme/team` too.
///
/// Chain membership is read off the names rather than fetched: a group's
/// name is its full path, so an ancestor of `acme/team` is exactly a group
/// whose name is a `/`-boundary prefix of it. `groups` is what
/// [`visible_groups`] returned, which already contains every ancestor the
/// viewer belongs to.
///
/// `pub` (not `pub(crate)`) so `tests/auth_web_groups.rs`, which links this
/// crate as an external consumer, can exercise the chain rule directly —
/// same reasoning as the `render_*` helpers around it.
pub fn manageable_group_names<'a>(groups: &'a [Group], viewer: &Uid, is_admin: bool) -> Vec<&'a str> {
    let owned: Vec<&str> = groups
        .iter()
        .filter(|g| g.role_of(viewer) >= Some(Permission::Owner))
        .map(|g| g.name.as_str())
        .collect();
    groups
        .iter()
        .filter(|g| {
            is_admin
                || owned
                    .iter()
                    .any(|owned| g.name == *owned || g.name.starts_with(&format!("{owned}/")))
        })
        .map(|g| g.name.as_str())
        .collect()
}

/// The path `add_group` is about to claim: a group's stored `name` is its
/// full path, so a subgroup's is its parent's plus the submitted segment.
/// Used for the optimistically-synthesized row and the "name already taken"
/// message, both of which have to name the same thing
/// `domain::group::create` did.
fn full_group_path(parent: &str, segment: &str) -> String {
    let (parent, segment) = (parent.trim(), segment.trim());
    if parent.is_empty() {
        segment.to_string()
    } else {
        format!("{parent}/{segment}")
    }
}

/// A uid-keyed [`Group::members`] entry resolved through `names` for display
/// — `None` means the uid no longer resolves (its user was deleted), which
/// gets a plain "(user deleted)" label and no removal form, since there's no
/// username left to resolve back to a uid on submit (see
/// `domain::group::remove_member`'s own comment — the orphaned entry is
/// harmless to leave, just not cleanable from here).
fn resolve_name<'a>(names: &'a HashMap<Uid, String>, uid: &Uid) -> Option<&'a str> {
    names.get(uid).map(String::as_str)
}

/// The visible group cards alone, wrapped in a stable `id` — every action
/// on this page (`data-async-target="#groups-list"`, see
/// `crate::webui::ASYNC_HEADER`'s own doc comment) swaps just this fragment
/// instead of re-rendering the whole page.
///
/// Membership, not ownership, is what puts a group on this list: a
/// `Developer` in a group sees it here (that's how they find the group at
/// all), just without the member-editing forms `render_group` gates on
/// `can_manage`.
fn render_groups_list(
    viewer: &Uid,
    is_admin: bool,
    groups: &[Group],
    names: &HashMap<Uid, String>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    // No membership filter here: `visible_groups` already decided what this
    // viewer may see, and a subgroup they reach through a parent's
    // membership has no member entry of its own to test.
    let visible: Vec<&Group> = groups.iter().collect();
    html! {
        div id="groups-list" {
            @if visible.is_empty() {
                div class="empty-state" { div class="empty-state-title" { (gitquarry_i18n::web::auth::groups::groups_empty_hint(locale)) } }
            }
            @for group in &visible {
                (render_group(group, viewer, is_admin, names, csrf, locale))
            }
        }
    }
}

pub fn render_group(
    group: &Group,
    viewer: &Uid,
    is_admin: bool,
    names: &HashMap<Uid, String>,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    // Same rule `access::can_manage_group` applies, re-derived here from the
    // group already in hand rather than refetched: rendering has no reason
    // to disagree with what the POST handler will accept.
    let can_manage = is_admin || group.role_of(viewer) >= Some(Permission::Owner);
    let members = group.effective_members();
    html! {
        div class="card card-pad user-card" {
            h4 { (group.name) }

            @if !group.legacy_owners.is_empty() || !group.legacy_teams.is_empty() {
                p class="muted" { (gitquarry_i18n::web::auth::groups::groups_legacy_pending_warning(locale)) }
            }

            form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                (PreEscaped(csrf))
                input type="hidden" name="group_action" value="set_description";
                input type="hidden" name="name" value=(group.name);
                input name="description" value=(group.description.as_deref().unwrap_or("")) size="60"
                    placeholder=(gitquarry_i18n::web::auth::groups::groups_description_placeholder(locale));
                button type="submit" { (gitquarry_i18n::web::auth::groups::groups_save_button(locale)) }
            }

            @if is_admin {
                form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                    (PreEscaped(csrf))
                    input type="hidden" name="group_action" value="set_storage_quota";
                    input type="hidden" name="name" value=(group.name);
                    input name="storage_quota" size="12" placeholder=(gitquarry_i18n::web::auth::users::storage_quota_placeholder(locale));
                    @if let Some(bytes) = group.storage_quota_bytes {
                        span class="muted" { (gitquarry_i18n::web::auth::users::storage_quota_current_prefix(locale)) (gitquarry_domain::commands::repack::human_bytes(bytes)) }
                    }
                    button type="submit" { (gitquarry_i18n::web::auth::users::set_storage_quota_button(locale)) }
                }
            }

            h4 style="border-bottom:none" { (gitquarry_i18n::web::auth::groups::groups_members_heading(locale)) }
            table class="list" {
                tr {
                    th { (gitquarry_i18n::web::auth::groups::groups_member_column(locale)) }
                    th { (gitquarry_i18n::web::auth::groups::groups_role_column(locale)) }
                    th {}
                }
                @if members.is_empty() {
                    tr { td colspan="3" { em { (gitquarry_i18n::web::auth::groups::groups_no_members_hint(locale)) } } }
                }
                @for (uid, perm) in &members {
                    @match resolve_name(names, uid) {
                        Some(username) => (member_row(&group.name, username, *perm, can_manage, csrf, locale)),
                        None => tr {
                            td { em { (gitquarry_i18n::shared::auth::user_deleted(locale)) } }
                            td { span class=(crate::webui::permission_badge_class(*perm)) { (crate::webui::permission_label(*perm, locale)) } }
                            td {}
                        },
                    }
                }
            }
            @if can_manage {
                form method="post" action="/_groups" data-async data-async-target="#groups-list" {
                    (PreEscaped(csrf))
                    input type="hidden" name="group_action" value="set_member";
                    input type="hidden" name="name" value=(group.name);
                    input name="username" size="30" placeholder=(gitquarry_i18n::web::auth::groups::groups_username_placeholder(locale));
                    (permission_select(Permission::Developer, locale))
                    button type="submit" { (gitquarry_i18n::web::auth::groups::groups_add_member_button(locale)) }
                }
            }

            @if is_admin {
                form method="post" action="/_groups" style="display:inline" data-async data-async-target="#groups-list" data-confirm=(gitquarry_i18n::web::auth::groups::groups_delete_group_confirm(locale, &group.name)) {
                    (PreEscaped(csrf))
                    input type="hidden" name="group_action" value="delete_group";
                    input type="hidden" name="name" value=(group.name);
                    button type="submit" { (gitquarry_i18n::web::auth::groups::groups_delete_group_button(locale)) }
                }
            }
        }
    }
}

/// Every [`Permission`] tier, low to high — shared by the intro prose (lists
/// them all), the add-member `<select>` and each existing member's own
/// change-role `<select>`.
const PERMISSION_TIERS: [Permission; 5] = [
    Permission::Guest,
    Permission::Reporter,
    Permission::Developer,
    Permission::Maintainer,
    Permission::Owner,
];

/// The `perm` field's wire value — the same five lowercase tier names
/// `domain::group::set_member` parses back. Deliberately not
/// `crate::webui::permission_label`, which is translated and would stop round-tripping
/// the moment the page renders in Spanish.
fn permission_value(perm: Permission) -> &'static str {
    match perm {
        Permission::Guest => "guest",
        Permission::Reporter => "reporter",
        Permission::Developer => "developer",
        Permission::Maintainer => "maintainer",
        Permission::Owner => "owner",
    }
}

fn permission_select(selected: Permission, locale: gitquarry_i18n::Locale) -> Markup {
    html! {
        select name="perm" {
            @for tier in PERMISSION_TIERS {
                option value=(permission_value(tier)) selected[tier == selected] { (crate::webui::permission_label(tier, locale)) }
            }
        }
    }
}

/// One member's row: their name, their current tier, and — only for a
/// viewer who may manage the group — a change-role `<select>` and a removal
/// button. `set_member` doubles as the change-role action: it overwrites the
/// map entry, so there's no separate "edit" state for this form to get
/// wrong.
fn member_row(
    group_name: &str,
    username: &str,
    perm: Permission,
    can_manage: bool,
    csrf: &str,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    html! {
        tr {
            td { (username) }
            td {
                @if can_manage {
                    form method="post" action="/_groups" style="display:inline" data-async data-async-target="#groups-list" {
                        (PreEscaped(csrf))
                        input type="hidden" name="group_action" value="set_member";
                        input type="hidden" name="name" value=(group_name);
                        input type="hidden" name="username" value=(username);
                        (permission_select(perm, locale))
                        button type="submit" { (gitquarry_i18n::web::auth::groups::groups_set_role_button(locale)) }
                    }
                } @else {
                    span class=(crate::webui::permission_badge_class(perm)) { (crate::webui::permission_label(perm, locale)) }
                }
            }
            td {
                @if can_manage {
                    form method="post" action="/_groups" style="display:inline" data-async data-async-target="#groups-list" data-confirm=(gitquarry_i18n::web::auth::groups::groups_remove_member_confirm(locale, username)) {
                        (PreEscaped(csrf))
                        input type="hidden" name="group_action" value="remove_member";
                        input type="hidden" name="name" value=(group_name);
                        input type="hidden" name="username" value=(username);
                        button type="submit" { (gitquarry_i18n::web::auth::groups::groups_remove_button(locale)) }
                    }
                }
            }
        }
    }
}
