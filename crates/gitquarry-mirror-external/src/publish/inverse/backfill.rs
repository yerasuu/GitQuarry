//! Rebuilds an epoch's inverse index from its forward map, for an epoch
//! published before the index existed
//! (`docs/publish-mirror-inbound-concept.md` §3.2, "Backfill").
//!
//! The forward map holds every private commit the epoch's runs rewrote,
//! collapsed ones included, with the public commit each maps to: exactly
//! what those runs would have added to the inverse through
//! [`super::fresh_entries`]. Inverting it therefore gives the same union
//! without rewriting anything, without the private repo, and without
//! depending on private commits that may since have been force-pushed away.
//!
//! Additive like every inverse write: what a shard already holds is kept,
//! so a backfill over an epoch whose runs already wrote the index adds
//! nothing and rewrites no shard.

use super::{Shard, prefix, read_shard, shard_key, store};
use crate::publish::mapping::{LOAD_CONCURRENCY, Table, epoch_prefix, from_map_value};
use anyhow::Result;
use futures::{StreamExt, TryStreamExt};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::uid::Uid;
use std::collections::BTreeSet;

/// What [`plan`] found for one epoch; [`apply`] writes it.
#[derive(Debug)]
pub struct Backfill {
    prefix: String,
    missing: Shard,
    mapped: usize,
}

impl Backfill {
    /// Private commits of the forward map that have a public commit. Those
    /// that collapsed away with all their ancestors have none and belong in
    /// no inverse entry.
    pub fn mapped(&self) -> usize {
        self.mapped
    }

    /// Private commits among [`Self::mapped`] that the inverse doesn't list
    /// under their public commit yet.
    pub fn missing(&self) -> usize {
        self.missing.values().map(|privates| privates.len()).sum()
    }
}

/// Compares epoch `epoch` of mirror `mirror_id` of repo `uid` with its
/// forward map and returns what the inverse lacks. Reads only; an epoch
/// with no forward map plans nothing.
///
/// # Errors
///
/// Fails on a bucket error, or on a forward-map or inverse shard that
/// doesn't open under the bucket's key or doesn't decode: a corrupt shard
/// is never planned over.
pub async fn plan(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, mirror_id: &str, epoch: u32) -> Result<Backfill> {
    let epoch_prefix = epoch_prefix(uid, mirror_id, epoch);
    let mut map = Table::<40>::new(format!("{epoch_prefix}/map"));
    map.load_all(backend, cipher).await?;
    let mut wanted = Shard::new();
    let mut mapped = 0;
    for (private, value) in map.entries() {
        if let Some((public, _tree)) = from_map_value(value) {
            wanted.entry(public).or_default().insert(*private);
            mapped += 1;
        }
    }

    let prefix = prefix(&epoch_prefix);
    let shards: BTreeSet<u8> = wanted.keys().map(|public| public[0]).collect();
    let stored: Vec<Shard> = futures::stream::iter(shards)
        .map(|shard| {
            let key = shard_key(&prefix, shard);
            async move { Ok::<_, anyhow::Error>(read_shard(backend, cipher, &key).await?.0) }
        })
        .buffer_unordered(LOAD_CONCURRENCY)
        .try_collect()
        .await?;
    for (public, listed) in stored.iter().flatten() {
        if let Some(privates) = wanted.get_mut(public) {
            privates.retain(|private| !listed.contains(private));
        }
    }
    wanted.retain(|_, privates| !privates.is_empty());
    Ok(Backfill {
        prefix,
        missing: wanted,
        mapped,
    })
}

/// Adds what `backfill` found missing to the inverse. Safe to run while a
/// publish run of the same epoch writes the index too: both are unions
/// under CAS, and their entries agree.
///
/// # Errors
///
/// Fails on a corrupt shard, a bucket error, or when another writer keeps
/// winning the CAS race on one shard.
pub async fn apply(backend: &dyn Backend, cipher: &Cipher, backfill: &Backfill) -> Result<()> {
    store(backend, cipher, &backfill.prefix, &backfill.missing).await
}
