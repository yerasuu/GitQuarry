//! What a history publish keeps in the bucket between runs
//! (`docs/publish-mirror-concept.md` §3, "Mapping storage"), all of it per
//! mirror and per epoch under `repos/<uid>/publish/<mirror_id>/<epoch>/`:
//!
//! - `map/<xx>`: private commit → (public commit, public tree), sharded by
//!   the first byte of the private id. The tree rides along so a run can
//!   tell whether a new commit collapses onto its parent without reading
//!   the parent back out of the publish repo.
//! - `excluded/<xx>`: every blob the rewrite met at an excluded path, the
//!   history-wide half of the blob-set audit (note #46) that an incremental
//!   run would otherwise only know for its own new commits.
//! - `inverse/<xx>`: public commit → the private commits that map to it,
//!   for the inbound side ([`super::inverse`]).
//! - `objects/pack/`: the publish repo itself, one pack per run, through the
//!   same [`Remote`] machinery as a repo's own packs.
//!
//! Bulk keyed by sha and never queried otherwise, so these are plain sealed
//! objects on the `Remote` pattern rather than s3db entities. Every object
//! is sealed with its own key as AAD, like everything else in the bucket.
//! Nothing here deletes: an abandoned epoch stays until a GC sweep, for
//! rollback.

use super::objects::Oid;
use anyhow::{Context, Result, bail};
use futures::{StreamExt, TryStreamExt};
use gitquarry_core::app::AppContext;
use gitquarry_core::crypto::CompressionStrategy;
use gitquarry_core::storage::remote::Remote;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{Cas, REPOS_PREFIX, list_all_keys};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Shards fetched at once. A full rewrite reads all 256; one at a time that
/// is 256 sequential round trips to the bucket.
const LOAD_CONCURRENCY: usize = 16;

/// CAS attempts on one shard before a run gives up. Runs of one mirror are
/// serialised by the sweep lock, so a conflict means another node; the
/// entries are deterministic, so a retry only ever adds the same values.
pub(super) const STORE_ATTEMPTS: u32 = 8;

/// Bytes of a stored shard before its entries: a format tag, so a future
/// layout is refused instead of misread.
const MAGIC: &[u8; 4] = b"GQS1";

/// The key prefix of one mirror's epoch, without a trailing `/`.
pub(crate) fn epoch_prefix(uid: &Uid, mirror_id: &str, epoch: u32) -> String {
    format!("{REPOS_PREFIX}/{uid}/publish/{mirror_id}/{epoch}")
}

/// The publish repo's packs for one epoch.
pub(crate) fn pack_store(ctx: &AppContext, uid: &Uid, mirror_id: &str, epoch: u32) -> Remote {
    Remote::new(
        ctx.backend(),
        epoch_prefix(uid, mirror_id, epoch),
        std::sync::Arc::clone(&ctx.cipher),
    )
}

/// A private commit's public commit and its tree; both zero when the commit
/// and all its ancestors collapsed away.
pub(crate) type MapValue = [u8; 40];

pub(crate) fn map_value(public: Option<(Oid, Oid)>) -> MapValue {
    let mut value = [0; 40];
    if let Some((commit, tree)) = public {
        value[..20].copy_from_slice(&commit);
        value[20..].copy_from_slice(&tree);
    }
    value
}

pub(crate) fn from_map_value(value: &MapValue) -> Option<(Oid, Oid)> {
    if value.iter().all(|b| *b == 0) {
        return None;
    }
    let mut commit = [0; 20];
    let mut tree = [0; 20];
    commit.copy_from_slice(&value[..20]);
    tree.copy_from_slice(&value[20..]);
    Some((commit, tree))
}

/// One sharded table, `N` bytes of value per 20-byte key. Shards are loaded
/// on demand and cached for the run; a key whose shard isn't loaded answers
/// `None` like an absent one, so a caller loads every shard it will ask
/// about first.
pub(crate) struct Table<const N: usize> {
    prefix: String,
    shards: HashMap<u8, BTreeMap<Oid, [u8; N]>>,
}

impl<const N: usize> Table<N> {
    /// `prefix` is the table's key prefix, e.g. `<epoch prefix>/map`.
    pub(crate) fn new(prefix: String) -> Self {
        Self {
            prefix,
            shards: HashMap::new(),
        }
    }

    fn key(&self, shard: u8) -> String {
        format!("{}/{shard:02x}", self.prefix)
    }

    /// The shards that exist in the bucket, from one listing: lets a run
    /// skip a GET for every shard nobody ever wrote.
    pub(crate) async fn existing(&self, ctx: &AppContext) -> Result<BTreeSet<u8>> {
        let prefix = format!("{}/", self.prefix);
        Ok(list_all_keys(ctx.backend().as_ref(), &prefix)
            .await?
            .iter()
            .filter_map(|key| key.strip_prefix(&prefix))
            .filter_map(|name| u8::from_str_radix(name, 16).ok().filter(|_| name.len() == 2))
            .collect())
    }

    /// Loads every shard of `wanted` not loaded yet. A shard missing from
    /// the bucket loads as empty.
    pub(crate) async fn load(&mut self, ctx: &AppContext, wanted: impl IntoIterator<Item = u8>) -> Result<()> {
        let missing: BTreeSet<u8> = wanted.into_iter().filter(|s| !self.shards.contains_key(s)).collect();
        let loaded: Vec<(u8, BTreeMap<Oid, [u8; N]>)> = futures::stream::iter(missing)
            .map(|shard| {
                let key = self.key(shard);
                async move {
                    let (entries, _etag) = read_shard::<N>(ctx, &key).await?;
                    Ok::<_, anyhow::Error>((shard, entries))
                }
            })
            .buffer_unordered(LOAD_CONCURRENCY)
            .try_collect()
            .await?;
        self.shards.extend(loaded);
        Ok(())
    }

    /// Every shard that exists in the bucket.
    pub(crate) async fn load_all(&mut self, ctx: &AppContext) -> Result<()> {
        let existing = self.existing(ctx).await?;
        self.load(ctx, existing).await?;
        // The rest exist nowhere: loaded as empty, so a later `load` doesn't
        // ask the bucket for them again.
        for shard in 0..=u8::MAX {
            self.shards.entry(shard).or_default();
        }
        Ok(())
    }

    pub(crate) fn get(&self, key: &Oid) -> Option<&[u8; N]> {
        self.shards.get(&key[0])?.get(key)
    }

    /// Adds `entries` to the bucket, shard by shard under CAS, and to the
    /// loaded view. A key already stored with a different value is a hard
    /// error: within one epoch the rewrite is deterministic, so two values
    /// for one private id mean the inputs changed without an epoch bump.
    ///
    /// # Errors
    ///
    /// Fails on such a conflict, on a bucket error, or when another writer
    /// keeps winning the CAS race past [`STORE_ATTEMPTS`].
    pub(crate) async fn store(&mut self, ctx: &AppContext, entries: &BTreeMap<Oid, [u8; N]>) -> Result<()> {
        let mut by_shard: BTreeMap<u8, Vec<(&Oid, &[u8; N])>> = BTreeMap::new();
        for (key, value) in entries {
            by_shard.entry(key[0]).or_default().push((key, value));
        }
        for (shard, new) in by_shard {
            let key = self.key(shard);
            let merged = store_shard::<N>(ctx, &key, &new).await?;
            self.shards.insert(shard, merged);
        }
        Ok(())
    }
}

async fn store_shard<const N: usize>(
    ctx: &AppContext,
    key: &str,
    new: &[(&Oid, &[u8; N])],
) -> Result<BTreeMap<Oid, [u8; N]>> {
    for _ in 0..STORE_ATTEMPTS {
        let (mut entries, etag) = read_shard::<N>(ctx, key).await?;
        let mut changed = false;
        for (oid, value) in new {
            match entries.get(*oid) {
                Some(stored) if stored == *value => {}
                Some(_) => bail!(
                    "{key}: {} is already mapped to something else in this epoch; the rewrite's inputs changed \
                     without an epoch bump",
                    super::objects::to_hex(oid)
                ),
                None => {
                    entries.insert(**oid, **value);
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(entries);
        }
        let sealed = ctx
            .cipher
            .seal(key, &encode::<N>(&entries), CompressionStrategy::None)?;
        match ctx.backend().put_cas(key, sealed, etag.as_deref()).await? {
            Cas::Ok => return Ok(entries),
            Cas::Conflict => continue,
        }
    }
    bail!("{key}: lost the write race {STORE_ATTEMPTS} times in a row")
}

async fn read_shard<const N: usize>(ctx: &AppContext, key: &str) -> Result<(BTreeMap<Oid, [u8; N]>, Option<String>)> {
    let (sealed, etag) = ctx.backend().get(key).await?;
    if etag.is_none() {
        return Ok((BTreeMap::new(), None));
    }
    let plain = ctx.cipher.open(key, &sealed)?;
    Ok((
        decode::<N>(&plain).with_context(|| format!("{key} is not a valid shard"))?,
        etag,
    ))
}

fn encode<const N: usize>(entries: &BTreeMap<Oid, [u8; N]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAGIC.len() + entries.len() * (20 + N));
    out.extend_from_slice(MAGIC);
    for (key, value) in entries {
        out.extend_from_slice(key);
        out.extend_from_slice(value);
    }
    out
}

fn decode<const N: usize>(bytes: &[u8]) -> Result<BTreeMap<Oid, [u8; N]>> {
    let Some(body) = bytes.strip_prefix(MAGIC.as_slice()) else {
        bail!("unknown shard format");
    };
    let mut entries = BTreeMap::new();
    let mut rest = body;
    while let Some((key, after)) = rest.split_first_chunk::<20>() {
        let (value, after) = after.split_first_chunk::<N>().context("truncated shard")?;
        entries.insert(*key, *value);
        rest = after;
    }
    if !rest.is_empty() {
        bail!("truncated shard");
    }
    Ok(entries)
}
