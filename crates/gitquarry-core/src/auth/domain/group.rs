//! Every mutation of a [`crate::auth::entities::Group`] goes through here.
//! See [`crate::domain::repo`]'s doc comment for the `domain`/`web`
//! boundary this follows.
//!
//! `set_description`/`set_storage_quota`/`set_member`/`remove_member` all
//! take `&mut Group` rather than fetching and CAS-writing themselves —
//! `auth::web::groups` does one fetch, one mutation, one `put_group` per
//! form submit, so a single edit never costs more than one CAS write
//! regardless of which of these it calls. `create`/`delete` are the
//! exception: there's no existing record to mutate.

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{Group, Permission};
use crate::storage::Cas;
use crate::storage::uid::Uid;
use anyhow::{Context, Result, bail};
use s3db::txn::{self, TxnOp};
use std::collections::{BTreeMap, BTreeSet};

/// Reserves the namespace (shared with [`crate::auth::entities::User`]
/// names) and writes the group object in one create-only [`txn::run`]
/// transaction — same reasoning as [`crate::auth::domain::user::create`]: either
/// both land or neither does, no manual rollback-on-failure in between.
/// `None` if the resulting path is already taken (by a user, a group, or a
/// stray namespace entry).
///
/// `parent` nests this group inside an existing one: the stored `name`
/// becomes `<parent>/<segment>` — a group's name *is* its full path (see
/// [`Group`]'s doc comment) — and members of `parent` inherit their tier
/// here (`access::effective_permission`). `None` creates a top-level group.
///
/// `creator` starts as the group's sole [`Permission::Owner`] member —
/// without this, a group admin creates comes out with no members at all,
/// manageable only through the `is_admin` bypass
/// [`super::super::access::can_manage_group`] also grants; that's fine while
/// the creator stays a global admin, but silently strands the group the
/// moment they're demoted (nobody left who can add a member, since adding
/// one already needs to manage the group). Every other forge
/// (GitLab/GitHub/Codeberg groups and orgs, ...) makes the creator an owner
/// for the same reason.
pub async fn create(
    ctx: &AppContext,
    segment: &str,
    description: Option<String>,
    creator: Uid,
    parent: Option<&Group>,
    locale: gitquarry_i18n::Locale,
) -> Result<Option<Cas>> {
    access::validate_username(segment)?; // same charset rules as a username
    let name = match parent {
        Some(parent) => {
            // `+ 1` for the segment about to be appended: the cap is on the
            // resulting path, and past it `ancestor_group_names` would stop
            // short of the new group, silently dropping its own members from
            // every permission check on its repos.
            if access::ancestor_group_names(&parent.name).len() + 1 > access::MAX_GROUP_DEPTH {
                bail!(
                    "{}",
                    gitquarry_i18n::shared::auth::group_nesting_too_deep(locale, access::MAX_GROUP_DEPTH)
                );
            }
            format!("{}/{segment}", parent.name)
        }
        None => segment.to_string(),
    };
    // A group path and a repo path share one URL space (`/<namespace>/<name>`),
    // so a subgroup can't take a name a repo already holds — the namespace
    // reservation below only guards against other users/groups, which is the
    // other half of the same rule (`access::namespace_reserved`, checked by
    // `AppContext::create_repo`).
    if crate::storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, &name)
        .await?
        .is_some()
    {
        return Ok(None);
    }
    let group = Group {
        uid: Uid::new(),
        name,
        description,
        members: BTreeMap::from([(creator, Permission::Owner)]),
        parent: parent.map(|p| p.uid.clone()),
        storage_quota_bytes: None,
        legacy_owners: BTreeSet::new(),
        legacy_teams: Vec::new(),
    };
    let ops = vec![
        TxnOp::create(
            access::namespace_reservation_key(&group.name)?,
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
    if let Some(parent_uid) = &group.parent {
        access::add_child_group_index(ctx.backend().as_ref(), &ctx.cipher, parent_uid, &group.uid).await;
    }
    Ok(Some(cas))
}

/// Best-effort: deletes the group object then releases its uid/namespace/
/// child index entries. A no-op if `name` doesn't exist.
///
/// `Err` while the group still has subgroups: their own names are paths
/// under this one (`<name>/<child>`), so freeing this namespace would let a
/// *different* user or group claim `name` and inherit membership over
/// descendants nobody meant to hand them. The operator deletes the
/// subgroups first — nothing here deletes a tree recursively, same as
/// nothing deletes a group's repos for it.
pub async fn delete(ctx: &AppContext, name: &str, locale: gitquarry_i18n::Locale) -> Result<()> {
    if let Some(group) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, name).await?.0 {
        let children = access::list_child_groups(ctx.backend().as_ref(), &ctx.cipher, &group.uid).await?;
        if !children.is_empty() {
            bail!(
                "{}",
                gitquarry_i18n::shared::auth::group_still_has_subgroups(locale, children.len())
            );
        }
        if let Some(parent_uid) = &group.parent {
            access::remove_child_group_index(ctx.backend().as_ref(), &ctx.cipher, parent_uid, &group.uid).await;
        }
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

/// Grants `username` the `permission_str` tier across the whole group —
/// also the way an existing member's tier is *changed*, since the map entry
/// is simply overwritten. `Err` if `username` doesn't name an existing user
/// or `permission_str` isn't one of the five [`Permission`] tier names.
pub async fn set_member(
    ctx: &AppContext,
    group: &mut Group,
    username: &str,
    permission_str: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    let permission = match permission_str {
        "guest" => Permission::Guest,
        "reporter" => Permission::Reporter,
        "developer" => Permission::Developer,
        "maintainer" => Permission::Maintainer,
        "owner" => Permission::Owner,
        other => bail!("{}", gitquarry_i18n::shared::auth::invalid_permission(locale, other)),
    };
    let user = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await?
        .0
        .with_context(|| gitquarry_i18n::shared::auth::user_not_found(locale, username))?;
    group.members.insert(user.uid, permission);
    Ok(())
}

/// Drops `username` from the group entirely. Also strips them out of the
/// legacy fields ([`Group::legacy_owners`]/[`Group::legacy_teams`]): those
/// still grant access until `migrate-group-members` runs, so removing only
/// the [`Group::members`] entry would leave a removed member with their old
/// tier intact — a silent no-op exactly where one is least acceptable.
///
/// No existence check, unlike [`set_member`]: if `username` was since
/// deleted there's no uid left to look up by name, but that's harmless — an
/// orphaned uid in `members` never matches anyone (`effective_permission`/
/// `can_manage_group` re-fetch the live `User` and compare `.uid`), nothing
/// needs cleaning up.
pub async fn remove_member(ctx: &AppContext, group: &mut Group, username: &str) -> Result<()> {
    if let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?.0 {
        group.members.remove(&user.uid);
        group.legacy_owners.remove(&user.uid);
        for team in &mut group.legacy_teams {
            team.members.remove(&user.uid);
        }
    }
    Ok(())
}
