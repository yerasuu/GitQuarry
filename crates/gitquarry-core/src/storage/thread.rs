//! Append-only, chronologically-keyed thread storage — shared by
//! `merge_requests` (comments, reviews) and the `gitquarry-tracker` crate's
//! own issue-comment storage. Extracted from `merge_requests` once a second
//! real consumer (issues) needed the exact same pattern; not a speculative
//! abstraction, a hoist. `pub`, not `pub(super)`: issues moved into
//! `gitquarry-tracker` (`docs/tracker-extension-concept.md`), a separate
//! crate, so `pub(super)` visibility (module-local to this crate) is no
//! longer enough for that consumer to reach these.

use super::{Backend, Cas};
use crate::crypto::Cipher;
use anyhow::{Result, bail};
use s3db::Collection;
use serde::Serialize;
use std::sync::Arc;

/// Same timestamp+random key scheme as `storage::audit::entry_key`, but
/// nanosecond- rather than millisecond-precision: two comments posted back
/// to back (a fast reviewer, or a test) routinely land in the same
/// millisecond, and audit's own random-suffix tiebreak on a tie is only
/// acceptable there because nothing asserts a strict order across it —
/// here, thread order is the entire point, so the timestamp itself needs
/// fine enough resolution that a same-repo tie essentially never happens in
/// practice, with the random suffix only as a last-resort tiebreak.
fn thread_key(at_nanos: u128) -> String {
    format!("{at_nanos:032}-{:08x}", rand::random::<u32>())
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Appends `value` under a fresh [`thread_key`]. Shared by every append-only,
/// chronologically-keyed collection hanging off an MR or issue, with the same
/// "a `Cas::Conflict` here can only mean a key collision" contract.
pub async fn append_to_thread<T: Serialize + Sync>(
    collection: &Collection,
    backend: &dyn Backend,
    cipher: &Cipher,
    value: &T,
    what: &str,
) -> Result<()> {
    let key = thread_key(now_nanos());
    match collection.put(backend, cipher, &key, value, None).await? {
        Cas::Ok => Ok(()),
        Cas::Conflict => {
            bail!("key collision writing a {what} (nanos+random collision, extremely unlikely)")
        }
    }
}

/// Every object in an append-only thread collection, oldest first, fetched
/// concurrently but re-sorted by the original (already-chronological)
/// listing order rather than by completion order — same reasoning as
/// `storage::audit::list_recent`. Each entry keeps its own storage key:
/// `mirror::external`'s Track B MR sync needs a stable per-comment identity
/// (`ExternalPrLink::synced_local_comment_keys`), and neither `MrComment` nor
/// `MrReview` carries an id of its own on the wire.
pub async fn list_thread<T: serde::de::DeserializeOwned + Default + Send + 'static>(
    collection: Collection,
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
) -> Result<Vec<(String, T)>> {
    let names = collection.list_names(backend.as_ref()).await?;

    let mut set = tokio::task::JoinSet::new();
    for (index, name) in names.into_iter().enumerate() {
        let collection = collection.clone();
        let backend = Arc::clone(backend);
        let cipher = Arc::clone(cipher);
        set.spawn(async move {
            let result = collection.get::<T>(backend.as_ref(), &cipher, &name).await;
            (index, name, result)
        });
    }
    let mut found = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok((index, key, Ok((Some(value), _etag)))) = result {
            found.push((index, key, value));
        }
    }
    found.sort_unstable_by_key(|(index, _, _)| *index);
    Ok(found.into_iter().map(|(_, key, value)| (key, value)).collect())
}
