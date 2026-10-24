//! The inverse of a history publish's mapping, per mirror and per epoch,
//! under `repos/<uid>/publish/<mirror_id>/<epoch>/inverse/<xx>`: public
//! commit → every private commit that maps to it, sharded by the first byte
//! of the public id. What the inbound side needs to turn a public pull
//! request's base back into a private commit
//! (`docs/publish-mirror-inbound-concept.md` §3.2).
//!
//! Not a [`super::mapping::Table`]: that one holds one value per key and
//! refuses a second, which is right for the forward map and wrong here.
//! Several private commits collapse onto one public commit (those that only
//! touch excluded paths, a merge whose sides collapse to one), and two
//! private commits can in principle hash to the same public one, so a key
//! holds a set and a store is a union.
//!
//! A shard is sealed with its key as AAD like everything else, and is
//! [`MAGIC`] followed by entries in strictly ascending public order, each
//! `public (20 bytes) | n (u32, big-endian, ≥ 1) | n private ids (20 bytes
//! each, strictly ascending)`. Anything else, including trailing bytes, is
//! refused as corrupt rather than read in part.

use super::mapping::{STORE_ATTEMPTS, epoch_prefix};
use super::objects::{Oid, from_hex, to_hex};
use super::rewrite::Rewriter;
use crate::validate::validate_sha;
use anyhow::{Context, Result, bail};
use gitquarry_core::app::AppContext;
use gitquarry_core::crypto::CompressionStrategy;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::uid::Uid;
use std::collections::{BTreeMap, BTreeSet};

/// Distinct from the forward map's tag, so a shard of one is never decoded
/// as the other.
const MAGIC: &[u8; 4] = b"GQI1";

type Shard = BTreeMap<Oid, BTreeSet<Oid>>;

/// The table's key prefix within an epoch, without a trailing `/`.
pub(crate) fn prefix(epoch_prefix: &str) -> String {
    format!("{epoch_prefix}/inverse")
}

fn shard_key(prefix: &str, shard: u8) -> String {
    format!("{prefix}/{shard:02x}")
}

/// The inverse entries of what `rewriter` rewrote in this run: each fresh
/// private commit under the public commit it maps to, collapsed ones under
/// the public commit they collapsed onto. A commit that collapsed away with
/// all its ancestors has no public commit and no entry.
pub(crate) fn fresh_entries(rewriter: &Rewriter) -> Shard {
    let mut entries = Shard::new();
    for private in &rewriter.fresh {
        if let Some(Some(public)) = rewriter.mapped(private) {
            entries.entry(public).or_default().insert(*private);
        }
    }
    entries
}

/// Adds `entries` to the table under `prefix`, shard by shard under CAS. A
/// union: what a shard already holds is kept, a private id already listed
/// under its public id is not written again, and a shard that gains nothing
/// is not rewritten, so storing the same entries twice changes nothing.
///
/// # Errors
///
/// Fails on a corrupt shard, a bucket error, or when another writer keeps
/// winning the CAS race past [`STORE_ATTEMPTS`].
pub(crate) async fn store(ctx: &AppContext, prefix: &str, entries: &Shard) -> Result<()> {
    let mut by_shard: BTreeMap<u8, Vec<(&Oid, &BTreeSet<Oid>)>> = BTreeMap::new();
    for (public, privates) in entries {
        by_shard.entry(public[0]).or_default().push((public, privates));
    }
    for (shard, new) in by_shard {
        store_shard(ctx, &shard_key(prefix, shard), &new).await?;
    }
    Ok(())
}

async fn store_shard(ctx: &AppContext, key: &str, new: &[(&Oid, &BTreeSet<Oid>)]) -> Result<()> {
    for _ in 0..STORE_ATTEMPTS {
        let (mut entries, etag) = read_shard(ctx, key).await?;
        let mut changed = false;
        for (public, privates) in new {
            let stored = entries.entry(**public).or_default();
            for private in *privates {
                changed |= stored.insert(*private);
            }
        }
        if !changed {
            return Ok(());
        }
        let sealed = ctx.cipher.seal(key, &encode(&entries)?, CompressionStrategy::None)?;
        match ctx.backend().put_cas(key, sealed, etag.as_deref()).await? {
            Cas::Ok => return Ok(()),
            Cas::Conflict => continue,
        }
    }
    bail!("{key}: lost the write race {STORE_ATTEMPTS} times in a row")
}

/// The private commits that mirror `mirror_id` of repo `uid` published as
/// public commit `public` within epoch `epoch`, as lowercase hex, sorted.
/// More than one when several private commits collapsed onto it.
///
/// Reads one shard of that epoch and nothing else: it never falls back to
/// another epoch, never reads the private repo or the forward map, and
/// never rebuilds a missing shard. An empty result means this epoch's
/// index has no such public commit, which is also what an epoch published
/// before the index existed answers for every commit.
///
/// # Errors
///
/// Fails when `public` isn't a 40-character lowercase hex id, on a bucket
/// error, when the shard doesn't open under the bucket's key, or when it
/// doesn't decode exactly (see the module doc): a corrupt shard is never
/// read in part.
pub async fn private_commits(
    ctx: &AppContext,
    uid: &Uid,
    mirror_id: &str,
    epoch: u32,
    public: &str,
) -> Result<Vec<String>> {
    validate_sha(public)?;
    let oid = from_hex(public)?;
    let key = shard_key(&prefix(&epoch_prefix(uid, mirror_id, epoch)), oid[0]);
    let (entries, _etag) = read_shard(ctx, &key).await?;
    Ok(entries
        .get(&oid)
        .map(|privates| privates.iter().map(to_hex).collect())
        .unwrap_or_default())
}

async fn read_shard(ctx: &AppContext, key: &str) -> Result<(Shard, Option<String>)> {
    let (sealed, etag) = ctx.backend().get(key).await?;
    if etag.is_none() {
        return Ok((Shard::new(), None));
    }
    let plain = ctx
        .cipher
        .open(key, &sealed)
        .with_context(|| format!("{key} doesn't open under this bucket's key"))?;
    Ok((
        decode(&plain).with_context(|| format!("{key} is not a valid inverse shard"))?,
        etag,
    ))
}

fn encode(entries: &Shard) -> Result<Vec<u8>> {
    let size = entries.values().map(|privates| 24 + 20 * privates.len()).sum::<usize>();
    let mut out = Vec::with_capacity(MAGIC.len() + size);
    out.extend_from_slice(MAGIC);
    for (public, privates) in entries {
        let count = u32::try_from(privates.len()).context("too many private commits under one public commit")?;
        out.extend_from_slice(public);
        out.extend_from_slice(&count.to_be_bytes());
        for private in privates {
            out.extend_from_slice(private);
        }
    }
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<Shard> {
    let Some(mut rest) = bytes.strip_prefix(MAGIC.as_slice()) else {
        bail!("unknown shard format");
    };
    let mut entries = Shard::new();
    let mut last_public = None;
    while !rest.is_empty() {
        let (public, after) = rest.split_first_chunk::<20>().context("truncated entry")?;
        let (count, mut after) = after.split_first_chunk::<4>().context("truncated entry")?;
        if last_public.is_some_and(|last| last >= *public) {
            bail!("public ids out of order");
        }
        let count = u32::from_be_bytes(*count);
        if count == 0 {
            bail!("{} lists no private commit", to_hex(public));
        }
        let mut privates = BTreeSet::new();
        let mut last_private = None;
        for _ in 0..count {
            let (private, next) = after.split_first_chunk::<20>().context("truncated entry")?;
            if last_private.is_some_and(|last| last >= *private) {
                bail!("private ids under {} out of order", to_hex(public));
            }
            privates.insert(*private);
            last_private = Some(*private);
            after = next;
        }
        entries.insert(*public, privates);
        last_public = Some(*public);
        rest = after;
    }
    Ok(entries)
}
