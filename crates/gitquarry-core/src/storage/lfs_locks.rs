//! Git LFS locking API metadata (`POST/GET .../info/lfs/locks`,
//! `POST .../info/lfs/locks/verify`, `POST .../info/lfs/locks/<id>/unlock`,
//! see <https://github.com/git-lfs/git-lfs/blob/main/docs/api/locking.md>) —
//! same per-repo `s3db` `Collection` convention as `issues`/`milestones`
//! (tracker, parked on `feature/tracker`): metadata scoped per-repo-uid at
//! runtime, so `#[derive(Entity)]`'s compile-time-literal collection name
//! can't be used. Separate from [`super::remote::Remote`]'s LFS object
//! methods on purpose: a lock is small JSON metadata about a path, never a
//! content-addressed blob.

use super::uid::Uid;
use super::{Backend, Cas, REPOS_PREFIX};
use crate::crypto::Cipher;
use anyhow::{Context, Result, bail};
use s3db::Collection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LfsLock {
    #[serde(default)]
    pub id: u64,
    /// Repo-relative path, validated with `validate::validate_tree_path` at
    /// the HTTP layer before this is ever written.
    #[serde(default)]
    pub path: String,
    /// The authenticated username that took the lock — never empty, an
    /// anonymous caller can't reach lock creation at all (it needs
    /// `Permission::Developer`, which always resolves to a real credential).
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub locked_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LockCounter {
    #[serde(default)]
    next: u64,
}

/// Reserved key in every repo's locks collection, never a valid lock id
/// (ids start at 1) — same convention as `issues::COUNTER_KEY`.
const COUNTER_KEY: &str = "_counter";

/// Same retry bound as `issues::COUNTER_MAX_RETRIES`.
const COUNTER_MAX_RETRIES: u32 = 8;

pub fn lfs_locks_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/lfs-locks"))
}

/// Mints the next sequential lock id for this repo — same CAS get/
/// increment/put retry loop as `issues::next_issue_number`, against its own
/// counter (lock ids are their own sequence, unrelated to issue/MR/milestone
/// numbers).
pub async fn next_lock_id(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<u64> {
    let collection = lfs_locks_collection(uid);
    for attempt in 1..=COUNTER_MAX_RETRIES {
        let (counter, etag) = collection.get::<LockCounter>(backend, cipher, COUNTER_KEY).await?;
        let id = counter.map_or(1, |c| c.next.max(1));
        let updated = LockCounter { next: id + 1 };
        match collection
            .put(backend, cipher, COUNTER_KEY, &updated, etag.as_deref())
            .await?
        {
            Cas::Ok => return Ok(id),
            Cas::Conflict if attempt < COUNTER_MAX_RETRIES => {
                super::cas_retry_backoff(attempt).await;
                continue;
            }
            Cas::Conflict => break,
        }
    }
    bail!("too many CAS conflicts minting a lock id (giving up after {COUNTER_MAX_RETRIES} attempts)")
}

pub async fn get_lock(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    id: u64,
) -> Result<(Option<LfsLock>, Option<String>)> {
    lfs_locks_collection(uid).get(backend, cipher, &id.to_string()).await
}

/// Create-only CAS (`expected_etag: None`) — the caller
/// (`git::lfs::create_lock_response`) has already checked no other lock
/// claims the same path, so a conflict here only ever means an id collision
/// (impossible with a freshly minted one) or a genuine race, both of which
/// should fail loudly rather than silently overwrite.
pub async fn put_lock(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, lock: &LfsLock) -> Result<Cas> {
    lfs_locks_collection(uid)
        .put(backend, cipher, &lock.id.to_string(), lock, None)
        .await
}

pub async fn delete_lock(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, id: u64) -> Result<()> {
    lfs_locks_collection(uid).delete(backend, cipher, &id.to_string()).await
}

/// Every lock currently held on this repo. Skips [`COUNTER_KEY`], same
/// reasoning as `issues::list_issues`. `Err` if any lock fails to load:
/// leaving one out would tell a client its path is free when it isn't.
pub async fn list_locks(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>, uid: &Uid) -> Result<Vec<LfsLock>> {
    let collection = lfs_locks_collection(uid);
    let names: Vec<String> = collection
        .list_names(backend.as_ref())
        .await?
        .into_iter()
        .filter(|name| name != COUNTER_KEY)
        .collect();

    let mut set = tokio::task::JoinSet::new();
    for name in names {
        let collection = collection.clone();
        let backend = Arc::clone(backend);
        let cipher = Arc::clone(cipher);
        set.spawn(async move { collection.get::<LfsLock>(backend.as_ref(), &cipher, &name).await });
    }
    let mut locks = Vec::new();
    while let Some(result) = set.join_next().await {
        // `None` is a lock unlocked between the listing and this read.
        if let (Some(lock), _etag) = result.context("lock read task failed")?? {
            locks.push(lock);
        }
    }
    locks.sort_unstable_by_key(|lock| lock.id);
    Ok(locks)
}
