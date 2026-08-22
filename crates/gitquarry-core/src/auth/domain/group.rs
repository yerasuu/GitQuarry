//! Every mutation of a [`crate::auth::entities::Group`]/[`crate::auth::entities::Team`]
//! goes through here. See [`crate::domain::repo`]'s doc comment for the
//! `domain`/`web` boundary this follows.
//!
//! `set_description`/`add_owner`/`remove_owner`/`add_team`/`delete_team`/
//! `add_team_member`/`remove_team_member` all take `&mut Group` rather than
//! fetching and CAS-writing themselves — `auth::web::groups` does one fetch, one
//! mutation, one `put_group` per form submit (same shape
//! `apply_group_action` already used), so a single edit never costs more
//! than one CAS write regardless of which of these it calls. `create`/
//! `delete` are the exception: there's no existing record to mutate.

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{Group, Permission, Team};
use crate::storage::Cas;
use crate::storage::uid::Uid;
use anyhow::{Context, Result, bail};
use s3db::txn::{self, TxnOp};
use std::collections::BTreeSet;

/// Reserves the namespace (shared with [`crate::auth::entities::User`]
/// names) and writes the group object in one create-only [`txn::run`]
/// transaction — same reasoning as [`crate::auth::domain::user::create`]: either
/// both land or neither does, no manual rollback-on-failure in between.
/// `None` if `name` is already taken (by a user, a group, or a stray
/// namespace entry).
///
/// `creator` starts as the group's sole owner — without this, a group
/// admin creates comes out with an empty `owners` set, manageable only
/// through the `is_admin` bypass [`super::super::access::can_manage_group`]
/// also grants; that's fine while the creator stays a global admin, but
/// silently strands the group the moment they're demoted (nobody left who
/// can add an owner, since adding one already needs to manage the group).
/// Every other forge (GitHub/Codeberg orgs, ...) makes the creator an
/// owner for the same reason.
pub async fn create(ctx: &AppContext, name: &str, description: Option<String>, creator: Uid) -> Result<Option<Cas>> {
    access::validate_username(name)?; // same charset rules as a username
    let group = Group {
        uid: Uid::new(),
        name: name.to_string(),
        description,
        owners: BTreeSet::from([creator]),
        teams: Vec::new(),
        storage_quota_bytes: None,
    };
    let ops = vec![
        TxnOp::create(
            access::NAMESPACE_INDEX.key(name)?,
            &access::NamespaceReservation {
                kind: "group".to_string(),
            },
        )?,
        group.create_op()?,
    ];
    let cas = txn::run(ctx.backend().as_ref(), &ctx.cipher, ops).await?;
    if cas == Cas::Conflict {
        return Ok(None);
    }
    let _ = group.claim_uid(ctx.backend().as_ref(), &ctx.cipher).await;
    Ok(Some(cas))
}

/// Best-effort: deletes the group object then releases its uid/namespace
/// index entries. A no-op if `name` doesn't exist.
pub async fn delete(ctx: &AppContext, name: &str) -> Result<()> {
    if let Some(group) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, name).await?.0 {
        let _ = group.release_uid(ctx.backend().as_ref()).await;
    }
    access::delete_group_object(ctx.backend().as_ref(), &ctx.cipher, name).await;
    access::release_namespace(ctx.backend().as_ref(), name).await;
    Ok(())
}

pub fn set_description(group: &mut Group, description: Option<String>) {
    group.description = description;
}

/// `quota_bytes: None` clears the limit (unlimited) — same shape
/// `domain::user::set_storage_quota` has for a personal namespace.
pub fn set_storage_quota(group: &mut Group, quota_bytes: Option<u64>) {
    group.storage_quota_bytes = quota_bytes;
}

/// `Err` if `username` doesn't name an existing user.
pub async fn add_owner(
    ctx: &AppContext,
    group: &mut Group,
    username: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    let user = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await?
        .0
        .with_context(|| gitquarry_i18n::shared::auth::user_not_found(locale, username))?;
    group.owners.insert(user.uid);
    Ok(())
}

/// No existence check, unlike [`add_owner`]: if `username` was since
/// deleted there's no uid left to look up by name, but that's harmless — an
/// orphaned uid in `owners` never matches anyone (`effective_permission`/
/// `can_manage_group` re-fetch the live `User` and compare `.uid`), nothing
/// needs cleaning up.
pub async fn remove_owner(ctx: &AppContext, group: &mut Group, username: &str) -> Result<()> {
    if let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?.0 {
        group.owners.remove(&user.uid);
    }
    Ok(())
}

/// `Err` if a team named `name` already exists on `group`, `permission_str`
/// isn't one of the five [`Permission`] tier names, or `repos_csv` names a
/// repo that doesn't exist (see [`parse_team_repos`]).
pub async fn add_team(
    ctx: &AppContext,
    group: &mut Group,
    name: &str,
    permission_str: &str,
    repos_csv: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    if group.teams.iter().any(|t| t.name == name) {
        bail!("{}", gitquarry_i18n::shared::auth::team_already_exists(locale, name));
    }
    let permission = match permission_str {
        "guest" => Permission::Guest,
        "reporter" => Permission::Reporter,
        "developer" => Permission::Developer,
        "maintainer" => Permission::Maintainer,
        "owner" => Permission::Owner,
        other => bail!("{}", gitquarry_i18n::shared::auth::invalid_permission(locale, other)),
    };
    let repos = parse_team_repos(ctx, &group.name, repos_csv, locale).await?;
    group.teams.push(Team {
        name: name.trim().to_string(),
        members: BTreeSet::new(),
        permission,
        repos,
    });
    Ok(())
}

pub fn delete_team(group: &mut Group, name: &str) {
    group.teams.retain(|t| t.name != name);
}

/// `Err` if `team` doesn't exist on `group` or `username` doesn't name an
/// existing user.
pub async fn add_team_member(
    ctx: &AppContext,
    group: &mut Group,
    team: &str,
    username: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    let user = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await?
        .0
        .with_context(|| gitquarry_i18n::shared::auth::user_not_found(locale, username))?;
    find_team_mut(group, team, locale)?.members.insert(user.uid);
    Ok(())
}

/// Same "no cleanup needed, harmless if it can't be found by name anymore"
/// reasoning as [`remove_owner`]. `Err` if `team` doesn't exist on `group`.
pub async fn remove_team_member(
    ctx: &AppContext,
    group: &mut Group,
    team: &str,
    username: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    if let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?.0 {
        find_team_mut(group, team, locale)?.members.remove(&user.uid);
    }
    Ok(())
}

fn find_team_mut<'a>(group: &'a mut Group, team: &str, locale: gitquarry_i18n::Locale) -> Result<&'a mut Team> {
    group
        .teams
        .iter_mut()
        .find(|t| t.name == team)
        .with_context(|| gitquarry_i18n::shared::auth::team_not_found(locale, team))
}

/// Resolves each comma-separated repo-local name (relative to `group_name`
/// — the part after `<group>/`, same convention the form field uses) to
/// that repo's uid, existence-validated against the repo-name index — a
/// real tightening from the old scheme, which accepted arbitrary text with
/// zero validation (unlike `protected_branches`, checked against the
/// manifest). Empty input still means "every repo in the group".
pub async fn parse_team_repos(
    ctx: &AppContext,
    group_name: &str,
    s: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Option<BTreeSet<Uid>>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let mut uids = BTreeSet::new();
    for local in s.split(',').map(str::trim).filter(|r| !r.is_empty()) {
        let full_name = format!("{group_name}/{local}");
        let uid = crate::storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, &full_name)
            .await?
            .with_context(|| gitquarry_i18n::shared::repo::repo_not_found(locale, &full_name))?;
        uids.insert(uid);
    }
    Ok(Some(uids))
}
