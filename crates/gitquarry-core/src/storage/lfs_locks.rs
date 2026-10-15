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

#[derive(Debug, Default, Serialize, Deserialize)]
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

/// Which lock id holds a path — the create-only CAS on this object is what
/// makes "one lock per path" atomic, since lock records themselves are
/// keyed by id and two creators racing on one path would otherwise both
/// pass a list-then-put check.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PathClaim {
    #[serde(default)]
    id: u64,
}

fn lock_paths_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/lfs-lock-paths"))
}

/// Base64 because a repo path carries `/`, which would split one claim
/// across key segments.
fn path_claim_name(path: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path)
}

pub enum CreatedLock {
    Created(LfsLock),
    /// Someone else's lock already holds the path.
    Held(LfsLock),
}

/// Takes `path` for `owner` under a freshly minted id, atomically per path.
///
/// The lock record is written before its path claim, so a claim always
/// points at a record that existed: a claim whose record is gone (a crash
/// mid-unlock) is stale and gets taken over by CAS against its own etag,
/// while a live one is returned as [`CreatedLock::Held`] and this call's
/// own record is removed again.
pub async fn create_lock(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    path: String,
    owner: String,
    locked_at: String,
) -> Result<CreatedLock> {
    let lock = LfsLock {
        id: next_lock_id(backend, cipher, uid).await?,
        path,
        owner,
        locked_at,
    };
    let locks = lfs_locks_collection(uid);
    match locks.put(backend, cipher, &lock.id.to_string(), &lock, None).await? {
        Cas::Ok => {}
        Cas::Conflict => bail!("lock id {} already taken", lock.id),
    }

    let claims = lock_paths_collection(uid);
    let claim_name = path_claim_name(&lock.path);
    let ours = PathClaim { id: lock.id };
    for attempt in 1..=COUNTER_MAX_RETRIES {
        if claims.put(backend, cipher, &claim_name, &ours, None).await? == Cas::Ok {
            return Ok(CreatedLock::Created(lock));
        }
        let (claim, etag) = claims.get::<PathClaim>(backend, cipher, &claim_name).await?;
        if let Some(claim) = claim {
            match get_lock(backend, cipher, uid, claim.id).await?.0 {
                Some(held) if held.path == lock.path => {
                    delete_lock_record(backend, cipher, uid, lock.id).await?;
                    return Ok(CreatedLock::Held(held));
                }
                _ => {
                    if claims.put(backend, cipher, &claim_name, &ours, etag.as_deref()).await? == Cas::Ok {
                        return Ok(CreatedLock::Created(lock));
                    }
                }
            }
        }
        super::cas_retry_backoff(attempt).await;
    }
    delete_lock_record(backend, cipher, uid, lock.id).await?;
    bail!("too many CAS conflicts claiming lock path (giving up after {COUNTER_MAX_RETRIES} attempts)")
}

/// Releases `lock`'s path claim before its record, so the path never looks
/// free while the record is still listed. The claim is only removed while
/// it still names this lock: a claim pointing elsewhere belongs to a lock
/// that took the path over after this one's claim went stale.
pub async fn delete_lock(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, lock: &LfsLock) -> Result<()> {
    let claims = lock_paths_collection(uid);
    let claim_name = path_claim_name(&lock.path);
    if let (Some(claim), _etag) = claims.get::<PathClaim>(backend, cipher, &claim_name).await?
        && claim.id == lock.id
    {
        claims.delete(backend, cipher, &claim_name).await?;
    }
    delete_lock_record(backend, cipher, uid, lock.id).await
}

async fn delete_lock_record(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, id: u64) -> Result<()> {
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
