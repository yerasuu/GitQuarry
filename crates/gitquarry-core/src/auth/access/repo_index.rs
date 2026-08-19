//! Per-repo index of which users have an explicit [`Permission`] grant —
//! lets a permission grid list only those users instead of scanning the
//! whole bucket.

use super::users::resolve_user_uid;
use crate::auth::entities::{Permission, User};
use crate::crypto::Cipher;
use crate::storage::uid::Uid;
use crate::storage::{Backend, INDEX_PREFIX};
use anyhow::Result;
use s3db::Collection;
use std::sync::Arc;
use tokio::task::JoinSet;

fn repo_permission_index(repo_uid: &Uid) -> Collection {
    Collection::new_owned(format!("{INDEX_PREFIX}/repo-permission/{repo_uid}"))
}

/// Marks `user_uid` as having *some* explicit `Permission` on `repo_uid` —
/// content unused, existence is the fact, same shape as
/// `storage::settings`'s repo-owner/repo-visible indexes. Lets
/// `webui::repo_settings`'s permission grid list only the users actually
/// granted something on a repo (see [`list_repo_permissions`]) instead of
/// every user in the bucket, the way the owner-transfer and move-repo
/// namespace dropdowns on that same page still need to (any user or group
/// is a valid destination for those, so there's no narrower set to index).
/// Best-effort, same tolerance as every other index here: a missed write
/// just hides a grant from the grid until the next permission change on
/// that repo touches this index again — never a correctness problem, since
/// `mirror::hook`/`git::ssh` check `User::repos` directly, not this index.
pub async fn add_repo_permission_index(backend: &dyn Backend, cipher: &Cipher, repo_uid: &Uid, user_uid: &Uid) {
    let _ = repo_permission_index(repo_uid)
        .put(backend, cipher, user_uid.as_str(), &(), None)
        .await;
}

pub async fn remove_repo_permission_index(backend: &dyn Backend, cipher: &Cipher, repo_uid: &Uid, user_uid: &Uid) {
    repo_permission_index(repo_uid)
        .delete(backend, cipher, user_uid.as_str())
        .await;
}

/// Drops every `repo_uid` permission-index entry at once — called from
/// `domain::repo::delete`, same "clean up this repo's own index entries"
/// step as the owner/visible indexes there. Doesn't touch the users' own
/// `repos` maps (a deleted repo's uid lingering in there is the same
/// harmless dead reference `forked_from` already tolerates elsewhere) —
/// just this repo's slice of the permission index.
pub async fn clear_repo_permission_index(backend: &dyn Backend, cipher: &Cipher, repo_uid: &Uid) -> Result<()> {
    let index = repo_permission_index(repo_uid);
    for name in index.list_names(backend).await? {
        index.delete(backend, cipher, &name).await;
    }
    Ok(())
}

/// Bare uids currently indexed under `repo_uid` — the listing half of
/// [`list_repo_permissions`], split out so `commands::backfill_repo_index`
/// can check "is this (repo, user) pair already indexed" without paying for
/// a `resolve_user_uid` on every one (it doesn't need the `User`, just
/// whether the pointer exists).
pub async fn list_repo_permission_uids(backend: &dyn Backend, repo_uid: &Uid) -> Result<Vec<Uid>> {
    Ok(repo_permission_index(repo_uid)
        .list_names(backend)
        .await?
        .into_iter()
        .map(Uid::from)
        .collect())
}

/// Every `User` with an explicit `Permission` on `repo_uid`, resolved via
/// [`add_repo_permission_index`] rather than a bucket-wide user scan — what
/// `webui::repo_settings`'s permission grid actually needs. A stale index
/// entry (a lost race on [`remove_repo_permission_index`]) resolves to a
/// user whose own `repos` map no longer has `repo_uid` at all; filtered out
/// below rather than trusted, same doctrine
/// [`super::users::resolve_user_uid`] already follows for its own hint.
pub async fn list_repo_permissions(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    repo_uid: &Uid,
) -> Result<Vec<(User, Permission)>> {
    let uids = list_repo_permission_uids(backend.as_ref(), repo_uid).await?;
    let mut set = JoinSet::new();
    for uid in uids {
        let backend = Arc::clone(backend);
        let cipher = Arc::clone(cipher);
        let repo_uid = repo_uid.clone();
        set.spawn(async move {
            let user = resolve_user_uid(backend.as_ref(), &cipher, &uid).await?;
            Ok::<_, anyhow::Error>(user.and_then(|u| u.repos.get(&repo_uid).copied().map(|p| (u, p))))
        });
    }
    let mut granted = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok(Some(pair))) = result {
            granted.push(pair);
        }
    }
    granted.sort_by(|a, b| a.0.username.cmp(&b.0.username));
    Ok(granted)
}
