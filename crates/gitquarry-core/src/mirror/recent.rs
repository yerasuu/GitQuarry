//! Bucket-side record of which repos were used recently.
//!
//! `mirrors_root` is a local cache and nothing guarantees it survives a
//! restart — a container without a volume, a fresh host, an operator
//! wiping it. [`super::gc::prewarm_recent_mirrors`] used to answer "which
//! repos were people using?" by listing that very directory, so in exactly
//! the case a prewarm exists for (an empty mirrors dir, every first request
//! paying for a full sync from S3) it found nothing and did nothing.
//!
//! One small object holds the answer instead: the last [`RECENT_CAP`] repo
//! uids anyone opened, most recent first. A single GET at startup, so the
//! warm set is known before any mirror exists on disk.

use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::uid::Uid;
use anyhow::Result;
use s3db::{Singleton, Update};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RECENT_MIRRORS: Singleton = Singleton::new(".conf/index/mirrors-recent.json");

/// How many repos the index remembers. A prewarm syncs every one of them
/// at startup, so this is a bound on that burst of pack downloads as much
/// as on the object's size: past a few dozen repos, warming the tail costs
/// more bandwidth than the first request to it would have.
const RECENT_CAP: usize = 64;

/// How long one process waits before recording the same repo again. Same
/// reasoning as `gc`'s own last-access marker debounce: the index is only ever read
/// against `mirror_max_age` (days), so finer resolution buys nothing and
/// would put a bucket read-modify-write on every page view of a busy repo.
const RECORD_DEBOUNCE: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct RecentMirrors {
    /// Most recently used first — [`record_use`] inserts at the front, so
    /// [`recent_uids`] doesn't have to order anything.
    #[serde(default)]
    entries: Vec<RecentMirror>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct RecentMirror {
    #[serde(default)]
    uid: Uid,
    /// Unix seconds, compared against `AppContext::mirror_max_age` — the
    /// same window `gc`'s sweep keeps a mirror on disk for, so the index
    /// never asks for a warm the sweep would immediately undo.
    #[serde(default)]
    at: u64,
}

/// Repos this process already recorded, and when. Process-wide rather than
/// an `AppContext` field because it's a de-dupe for a best-effort index,
/// not state anything reads: a second bucket in the same process at worst
/// skips a write another one had just made.
static LAST_RECORDED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// `true` when `uid` hasn't been recorded within [`RECORD_DEBOUNCE`], and
/// stamps it as recorded now. Entries past the window are dropped on the
/// way through, so the map stays bounded by the repos actually in use in
/// the last quarter hour instead of every repo ever opened.
fn debounce_passes(uid: &str) -> bool {
    let mut recorded = LAST_RECORDED.lock().expect("recent-use debounce mutex poisoned");
    recorded.retain(|_, at| at.elapsed() < RECORD_DEBOUNCE);
    if recorded.contains_key(uid) {
        return false;
    }
    recorded.insert(uid.to_string(), Instant::now());
    true
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Records that `uid`'s mirror was just opened by a real request. Silent on
/// failure, and debounced per process: losing an entry costs one cold sync
/// after the next restart, never correctness, and this runs on the same
/// path a clone and every repo page take.
pub async fn record_use(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) {
    if !debounce_passes(uid.as_str()) {
        return;
    }
    let now = now_secs();
    let _ = RECENT_MIRRORS
        .update::<RecentMirrors, _>(backend, cipher, |recent| {
            recent.entries.retain(|e| e.uid != *uid);
            // Front, not "push and sort by `at`": two repos opened inside
            // the same second carry the same timestamp, and a sort would
            // then be free to keep the older one first. Insertion order is
            // the real order.
            recent.entries.insert(
                0,
                RecentMirror {
                    uid: uid.clone(),
                    at: now,
                },
            );
            recent.entries.truncate(RECENT_CAP);
            Ok(Update::Write)
        })
        .await;
}

/// The repos used within `max_age`, most recent first — what
/// [`super::gc::prewarm_recent_mirrors`] syncs at startup.
pub async fn recent_uids(backend: &dyn Backend, cipher: &Cipher, max_age: Duration) -> Result<Vec<Uid>> {
    let (recent, _etag) = RECENT_MIRRORS.get::<RecentMirrors>(backend, cipher).await?;
    let cutoff = now_secs().saturating_sub(max_age.as_secs());
    Ok(recent
        .entries
        .into_iter()
        .filter(|e| e.at >= cutoff)
        .map(|e| e.uid)
        .collect())
}
