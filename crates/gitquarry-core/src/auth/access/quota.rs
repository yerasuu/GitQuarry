//! Per-owner storage quota: `User::storage_quota_bytes`/
//! `Group::storage_quota_bytes` (`None` = unlimited, the default — no quota
//! unless an admin sets one) capped against the summed `pack_bytes +
//! lfs_bytes` of every repo that owner has (`storage::settings::sum_owner_repo_bytes`).
//!
//! Deliberately a soft "you're already over, no more writes until you free
//! space" gate, checked once up front (`mirror::hook::run_pre_receive_hook`,
//! `git::http`'s LFS upload handler) — not an attempt to predict an
//! incoming push's exact size, which nothing in this codebase (or git
//! itself, before the pack is actually built) knows ahead of time anyway.
//! A push that lands while already under quota is never rejected mid-way
//! even if it pushes the owner over; the *next* one is.

use super::{resolve_group_uid, resolve_user_uid};
use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::entities::OwnerRef;
use crate::storage::settings::sum_owner_repo_bytes;
use anyhow::Result;

/// `owner`'s own configured limit, `None` if unset or `owner`'s `User`/
/// `Group` record no longer resolves (a stale `RepoConfig::owner` uid —
/// treated the same as "no limit" rather than an error, matching every
/// other stale-hint tolerance in this codebase).
async fn quota_limit(backend: &dyn Backend, cipher: &Cipher, owner: &OwnerRef) -> Result<Option<u64>> {
    Ok(match owner {
        OwnerRef::User(uid) => resolve_user_uid(backend, cipher, uid)
            .await?
            .and_then(|u| u.storage_quota_bytes),
        OwnerRef::Group(uid) => resolve_group_uid(backend, cipher, uid)
            .await?
            .and_then(|g| g.storage_quota_bytes),
    })
}

/// `None` if the push/upload may proceed (no owner, no limit set, or still
/// under it); `Some(used, limit)` in bytes if `owner` is already at or over
/// their configured quota — the caller renders that pair into whatever
/// error shape its own transport needs (`mirror::hook` prints plain text,
/// `git::http` returns a bare 403 like it already does for an archived
/// repo).
pub async fn check_quota(
    backend: &dyn Backend,
    cipher: &Cipher,
    owner: Option<&OwnerRef>,
) -> Result<Option<(u64, u64)>> {
    let Some(owner) = owner else {
        return Ok(None);
    };
    let Some(limit) = quota_limit(backend, cipher, owner).await? else {
        return Ok(None);
    };
    let used = sum_owner_repo_bytes(backend, cipher, owner).await?;
    Ok((used >= limit).then_some((used, limit)))
}
