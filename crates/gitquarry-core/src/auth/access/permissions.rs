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
/// display path, for namespace/group/team matching, and its stable
/// `repo_uid`, for the explicit-grant/default lookups on `user.repos`),
/// given their own `User` record and — only if `repo`'s namespace names a
/// [`Group`] — that one group's record. Resolves namespace/group/team
/// inheritance: an explicit per-repo grant on the user wins first; then —
/// per this module's "namespace is a username or a group, never both"
/// invariant — implicit `Owner` if `repo`'s namespace *is* `username`
/// itself (their own personal repo, same as creating it through `/_new`
/// already assumes — they can delete/transfer it, not just push); else,
/// implicit `Owner` if `username` owns the group,
/// else the highest [`Permission`] among the group's teams `username`
/// belongs to that also cover this repo (`Team::repos` unset means "every
/// repo in the group"); then the user's own `default_permission`. Pure/sync
/// so it stays unit-testable without a live per-entity fetch —
/// [`effective_permission_for`] is the async wrapper that actually fetches
/// `user`/`group`/`repo_uid` and calls this.
#[cfg(any(test, feature = "test-util"))]
pub fn effective_permission(user: &User, group: Option<&Group>, repo: &str, repo_uid: &Uid) -> Option<Permission> {
    if let Some(perm) = user.repos.get(repo_uid) {
        return Some(*perm);
    }
    if let Some((namespace, _repo_local)) = repo.split_once('/') {
        if namespace == user.username {
            return Some(Permission::Owner);
        }
        if let Some(group) = group {
            if group.owners.contains(&user.uid) {
                return Some(Permission::Owner);
            }
            let team_perm = group
                .teams
                .iter()
                .filter(|t| t.members.contains(&user.uid))
                .filter(|t| t.repos.as_ref().is_none_or(|set| set.contains(repo_uid)))
                .map(|t| t.permission)
                .max();
            if team_perm.is_some() {
                return team_perm;
            }
        }
    }
    user.default_permission
}
#[cfg(not(any(test, feature = "test-util")))]
fn effective_permission(user: &User, group: Option<&Group>, repo: &str, repo_uid: &Uid) -> Option<Permission> {
    if let Some(perm) = user.repos.get(repo_uid) {
        return Some(*perm);
    }
    if let Some((namespace, _repo_local)) = repo.split_once('/') {
        if namespace == user.username {
            return Some(Permission::Owner);
        }
        if let Some(group) = group {
            if group.owners.contains(&user.uid) {
                return Some(Permission::Owner);
            }
            let team_perm = group
                .teams
                .iter()
                .filter(|t| t.members.contains(&user.uid))
                .filter(|t| t.repos.as_ref().is_none_or(|set| set.contains(repo_uid)))
                .map(|t| t.permission)
                .max();
            if team_perm.is_some() {
                return team_perm;
            }
        }
    }
    user.default_permission
}

/// `true` if `user` can manage `group` itself — add/remove owners,
/// create/delete teams, edit team membership/scope. Global admins always
/// can, without needing to be listed in `owners` too. Distinct from repo
/// *access* inside the group, which goes through [`effective_permission`]
/// instead. Pure/sync, same reasoning as [`effective_permission`] —
/// [`can_manage_group_for`] is the async wrapper.
#[cfg(any(test, feature = "test-util"))]
pub fn can_manage_group(user: Option<&User>, group: Option<&Group>) -> bool {
    if user.is_some_and(|u| u.is_admin) {
        return true;
    }
    user.is_some_and(|u| group.is_some_and(|g| g.owners.contains(&u.uid)))
}
#[cfg(not(any(test, feature = "test-util")))]
fn can_manage_group(user: Option<&User>, group: Option<&Group>) -> bool {
    if user.is_some_and(|u| u.is_admin) {
        return true;
    }
    user.is_some_and(|u| group.is_some_and(|g| g.owners.contains(&u.uid)))
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

/// `user`'s effective permission over a whole namespace (`owner`: a user's
/// own namespace or a group they belong to) rather than one specific repo —
/// what `gitquarry-artifacts` needs to authorize a package publish, since a
/// package belongs to a namespace, not any one repo
/// (`docs/artifacts-concept.md`). `Owner` if `owner` is the user themself or
/// a group they own; else the highest [`Permission`] among the group's
/// *org-wide* teams (`Team::repos: None` — a team scoped to specific repos
/// doesn't grant namespace-wide publish rights) `user` belongs to.
/// Deliberately **no** `user.default_permission` fallback, unlike
/// [`effective_permission`]'s per-repo resolution: that would let any user
/// publish under any namespace by default, which a per-repo default
/// permission was never meant to authorize.
pub async fn effective_namespace_permission_for_user(
    backend: &dyn Backend,
    cipher: &Cipher,
    user: &User,
    owner: &crate::storage::entities::OwnerRef,
) -> Result<Option<Permission>> {
    match owner {
        crate::storage::entities::OwnerRef::User(uid) => Ok((*uid == user.uid).then_some(Permission::Owner)),
        crate::storage::entities::OwnerRef::Group(uid) => {
            let Some(group) = super::groups::resolve_group_uid(backend, cipher, uid).await? else {
                return Ok(None);
            };
            if group.owners.contains(&user.uid) {
                return Ok(Some(Permission::Owner));
            }
            Ok(group
                .teams
                .iter()
                .filter(|t| t.members.contains(&user.uid))
                .filter(|t| t.repos.is_none())
                .map(|t| t.permission)
                .max())
        }
    }
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
