//! Append-only, chronologically-keyed thread storage for issue comments —
//! duplicated from `gitquarry_core::storage::thread` rather than depending
//! on `gitquarry-core` for it (`docs/tracker-binary-split-concept.md`'s own
//! "cheap to duplicate a few lines" reasoning): fully generic over
//! `s3db::Backend`/`s3db_crypto::Cipher`/`s3db::Collection`, no
//! `gitquarry-core`-specific logic at all.

use anyhow::{Result, bail};
use s3db::{Backend, Cas, Collection};
use s3db_crypto::Cipher;
use serde::Serialize;
use std::sync::Arc;

fn thread_key(at_nanos: u128) -> String {
    format!("{at_nanos:032}-{:08x}", rand::random::<u32>())
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Appends `value` under a fresh [`thread_key`] — same "a `Cas::Conflict`
/// here can only mean a key collision" contract as core's own.
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
/// listing order rather than by completion order.
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
