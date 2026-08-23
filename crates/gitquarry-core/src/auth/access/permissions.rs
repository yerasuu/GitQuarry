//! The actual grant-resolution logic: what [`Permission`] a user effectively
//! has on a repo, and the coarser "can manage this at all" checks built on
//! top of it.

use super::groups::get_group;
use super::users::get_user;
use crate::auth::entities::{Group, Permission, User};
use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::uid::Uid;
use anyhow::Result;

/// `username`'s effective permission for `repo` (identified by both its
/// display path, for namespace/group matching, and its stable `repo_uid`,
/// for the explicit-grant/default lookups on `user.repos`), given their own
/// `User` record and — only if `repo`'s namespace names a [`Group`] — that
/// one group's record. Resolves namespace/group inheritance: an explicit
/// per-repo grant on the user wins first; then — per this module's
/// "namespace is a username or a group, never both" invariant — implicit
/// `Owner` if `repo`'s namespace *is* `username` itself (their own personal
/// repo, same as creating it through `/_new` already assumes — they can
/// delete/transfer it, not just push); else whatever the group grants them,
/// which is their [`Group::role_of`] tier (group-wide, GitLab-shaped) raised
/// by any legacy team still scoped to this one repo
/// ([`Group::legacy_repo_role`]); then the user's own `default_permission`.
/// Pure/sync so it stays unit-testable without a live per-entity fetch —
/// [`effective_permission_for`] is the async wrapper that actually fetches
/// `user`/`group`/`repo_uid` and calls this.
#[cfg(any(test, feature = "test-util"))]
pub fn effective_permission(user: &User, group: Option<&Group>, repo: &str, repo_uid: &Uid) -> Option<Permission> {
    effective_permission_impl(user, group, repo, repo_uid)
}
#[cfg(not(any(test, feature = "test-util")))]
fn effective_permission(user: &User, group: Option<&Group>, repo: &str, repo_uid: &Uid) -> Option<Permission> {
    effective_permission_impl(user, group, repo, repo_uid)
}
fn effective_permission_impl(user: &User, group: Option<&Group>, repo: &str, repo_uid: &Uid) -> Option<Permission> {
    if let Some(perm) = user.repos.get(repo_uid) {
        return Some(*perm);
    }
    if let Some((namespace, _repo_local)) = repo.split_once('/') {
        if namespace == user.username {
            return Some(Permission::Owner);
        }
        if let Some(group) = group {
            let inherited = [group.role_of(&user.uid), group.legacy_repo_role(&user.uid, repo_uid)]
                .into_iter()
                .flatten()
                .max();
            if inherited.is_some() {
                return inherited;
            }
        }
    }
    user.default_permission
}

/// `true` if `user` can manage `group` itself — add/remove members, change
/// a member's tier, edit the group's own settings. That is exactly
/// "[`Permission::Owner`] in [`Group::members`]", not a separate owner list:
/// group-wide `Owner` already implies delete/transfer on every repo in the
/// group (see [`can_manage_repo_members`]), so a second, weaker notion of
/// "runs the group" would have nothing left to withhold. Global admins
/// always can, without being a member at all. Distinct from repo *access*
/// inside the group, which goes through [`effective_permission`] instead.
/// Pure/sync, same reasoning as [`effective_permission`] —
/// [`can_manage_group_for`] is the async wrapper.
#[cfg(any(test, feature = "test-util"))]
pub fn can_manage_group(user: Option<&User>, group: Option<&Group>) -> bool {
    can_manage_group_impl(user, group)
}
#[cfg(not(any(test, feature = "test-util")))]
fn can_manage_group(user: Option<&User>, group: Option<&Group>) -> bool {
    can_manage_group_impl(user, group)
}
fn can_manage_group_impl(user: Option<&User>, group: Option<&Group>) -> bool {
    if user.is_some_and(|u| u.is_admin) {
        return true;
    }
    user.is_some_and(|u| group.is_some_and(|g| g.role_of(&u.uid) >= Some(Permission::Owner)))
}

/// `true` if `perm` (the caller's [`effective_permission`] on some repo)
/// authorizes managing that repo's settings — rename, description,
/// visibility, default branch, topics, protected branches/tags. Global
/// admins always can, regardless of `perm`.
pub fn can_manage_repo_settings(perm: Option<Permission>, is_admin: bool) -> bool {
    is_admin || perm >= Some(Permission::Maintainer)
}

/// `true` if `perm` authorizes the repo's most destructive/ownership-level
/// actions — delete, transfer, and granting/revoking any tier (including
/// `Maintainer`/`Owner`) on it to others. Global admins always can.
pub fn can_manage_repo_members(perm: Option<Permission>, is_admin: bool) -> bool {
    is_admin || perm >= Some(Permission::Owner)
}

/// `user`'s effective permission for `repo` — for a caller that already has
/// the acting `User` in hand (most do: it came from `login::current_user`
/// or an earlier `get_user` a few lines up) and would otherwise re-fetch
/// the exact same record by `username` right back through
/// [`effective_permission_for`]. Fetches nothing but (only if the repo's
/// namespace names a group) that one group's record, never the rest of the
/// bucket's users/groups. See [`effective_permission`] for the actual
/// decision logic.
pub async fn effective_permission_for_user(
    backend: &dyn Backend,
    cipher: &Cipher,
    user: &User,
    repo: &str,
) -> Result<Option<Permission>> {
    let namespace = repo.split_once('/').map(|(ns, _)| ns);
    let group = match namespace {
        Some(ns) => get_group(backend, cipher, ns).await?.0,
        None => None,
    };
    // `Uid::default()` (never a real repo's uid) if `repo` doesn't exist
    // yet — the explicit-grant/`default_permission` lookups on `user.repos`
    // just miss, same as today; namespace/group/team implicit grants (the
    // actual "can I push to create this" answer for a not-yet-existing
    // personal repo) don't depend on the repo existing at all, so
    // resolution failing here must never gate the whole function.
    let repo_uid = crate::storage::settings::resolve_repo_name(backend, cipher, repo)
        .await?
        .unwrap_or_default();
    Ok(effective_permission(user, group.as_ref(), repo, &repo_uid))
}

/// [`effective_permission_for_user`], for a caller that only has `username`
/// in hand (e.g. `git::ssh`'s auth path, before any `User` has been
/// fetched at all) and needs the one extra lookup that implies.
pub async fn effective_permission_for(
    backend: &dyn Backend,
    cipher: &Cipher,
    username: &str,
    repo: &str,
) -> Result<Option<Permission>> {
    let Some(user) = get_user(backend, cipher, username).await?.0 else {
        return Ok(None);
    };
    effective_permission_for_user(backend, cipher, &user, repo).await
}

/// Whether `username` can manage `group_name` itself — fetches just the
/// acting user's own record plus the one named group's. See
/// [`can_manage_group`] for the actual decision logic.
pub async fn can_manage_group_for(
    backend: &dyn Backend,
    cipher: &Cipher,
    username: &str,
    group_name: &str,
) -> Result<bool> {
    let user = get_user(backend, cipher, username).await?.0;
    let group = get_group(backend, cipher, group_name).await?.0;
    Ok(can_manage_group(user.as_ref(), group.as_ref()))
}

/// Whether `username` can manage `repo`'s settings. See
/// [`can_manage_repo_settings`] for the actual decision logic; reuses
/// [`effective_permission_for_user`] rather than re-deriving the
/// namespace/group resolution itself, and fetches `username`'s own record
/// once for both the `is_admin` check and the permission lookup.
pub async fn can_manage_repo_settings_for(
    backend: &dyn Backend,
    cipher: &Cipher,
    username: &str,
    repo: &str,
) -> Result<bool> {
    let Some(user) = get_user(backend, cipher, username).await?.0 else {
        return Ok(false);
    };
    let perm = effective_permission_for_user(backend, cipher, &user, repo).await?;
    Ok(can_manage_repo_settings(perm, user.is_admin))
}

/// Whether `username` can manage `repo`'s membership (delete/transfer/grant
/// any tier on it). See [`can_manage_repo_members`] for the actual decision
/// logic. Same single-fetch shape as [`can_manage_repo_settings_for`].
pub async fn can_manage_repo_members_for(
    backend: &dyn Backend,
    cipher: &Cipher,
    username: &str,
    repo: &str,
) -> Result<bool> {
    let Some(user) = get_user(backend, cipher, username).await?.0 else {
        return Ok(false);
    };
    let perm = effective_permission_for_user(backend, cipher, &user, repo).await?;
    Ok(can_manage_repo_members(perm, user.is_admin))
}
