//! Retention: the only part of this system that deletes anything.
//!
//! Off unless configured, and for one reason: the replica's whole value in
//! the "oops" case — a `repack` that dropped the wrong pack, a repo purged
//! by mistake, a malicious deletion — is that it still has what the source
//! no longer does. Propagating deletions eagerly would trade that away for
//! disk space.
//!
//! ## Why an orphan ledger
//!
//! "Delete destination keys the source no longer has, but only after N
//! days" needs to know *when* a key stopped existing at the source, and no
//! backend here exposes an object's age ([`crate::storage::Backend`]'s
//! listing is `(key, size)`). So the first pass that notices a key is
//! orphaned writes that down — one small entity per orphan under
//! [`ORPHAN_ROOT`] at the destination — and a later pass deletes the ones
//! whose note is old enough. An orphan that comes back (a pack re-uploaded,
//! a repo restored) has its note dropped instead, so the grace period always
//! measures continuous absence rather than "we saw this once".
//!
//! Orphans are rare by nature (a repack, a purge), so the ledger stays small
//! and cleans itself up.

use crate::backup::inventory::{self, Class};
use crate::backup::pager::{KeyPages, PAGE_SIZE};
use crate::backup::snapshot;
use crate::crypto::Cipher;
use crate::storage::Backend;
use anyhow::{Context, Result};
use s3db::Entity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Where the orphan ledger lives, at the *destination*. Classified
/// [`Class::Never`] by [`inventory::classify`] so that promoting a replica to
/// a live bucket and backing that up onward can't carry the ledger along as
/// real data.
pub const ORPHAN_ROOT: &str = ".conf/backup-orphans";

/// One destination key the source no longer has, and when that was first
/// noticed.
#[derive(Debug, Default, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/backup-orphans")]
pub struct Orphan {
    /// Hex sha256 of [`Self::key`]. The key itself can't be a key segment —
    /// it contains `/`, which would scatter one entity across a directory
    /// tree — and hashing gives a fixed-length segment with no escaping
    /// rules to get wrong.
    #[serde(default)]
    #[entity(key)]
    pub key_hash: String,
    /// The destination key this note is about, stored because a hash can't
    /// be reversed and the delete pass needs the real key.
    #[serde(default)]
    pub key: String,
    /// Unix seconds when a pass first found this key orphaned.
    #[serde(default)]
    pub first_seen: u64,
}

fn key_hash(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Default)]
pub struct PruneOutcome {
    /// Keys deleted at the destination.
    pub deleted: u64,
    /// Keys newly noted as orphaned (nothing deleted for these yet).
    pub noted: u64,
    /// Notes dropped because the key came back at the source.
    pub cleared: u64,
}

/// Notes destination keys the source no longer has, and deletes the ones
/// noted longer than `grace_secs` ago.
///
/// Never touches a key the source still has, and never deletes on the first
/// pass that sees an orphan — that's what the ledger is for. `grace_secs`
/// of 0 still requires two passes: the note has to exist before it can be
/// old enough.
pub async fn prune_replica(
    src: &Arc<dyn Backend>,
    dest: &Arc<dyn Backend>,
    dest_cipher: &Cipher,
    grace_secs: u64,
    now: u64,
) -> Result<PruneOutcome> {
    let mut outcome = PruneOutcome::default();

    // Same merge join as the copy pass, read the other way round: walk the
    // destination and ask what the source no longer has.
    let mut src_keys = KeyPages::new(Arc::clone(src), "");
    let mut dest_keys = KeyPages::new(Arc::clone(dest), "");
    let mut src_next = src_keys.next().await?;
    let mut orphans: Vec<String> = Vec::new();

    while let Some((dest_key, _size)) = dest_keys.next().await? {
        // The ledger, previous snapshots, and the backup system's own
        // records are not copies of anything at the source, so they'd look
        // orphaned to every pass. They have their own retention.
        if matches!(inventory::classify(&dest_key), Class::Never) || dest_key.starts_with(ORPHAN_ROOT) {
            continue;
        }
        while let Some((src_key, _)) = &src_next {
            if src_key < &dest_key {
                src_next = src_keys.next().await?;
            } else {
                break;
            }
        }
        match &src_next {
            Some((src_key, _)) if src_key == &dest_key => continue,
            _ => orphans.push(dest_key),
        }
    }

    let mut to_delete: Vec<String> = Vec::new();
    for key in &orphans {
        let name = key_hash(key);
        let (existing, _etag) = Orphan::COLLECTION
            .get::<Orphan>(dest.as_ref(), dest_cipher, &name)
            .await?;
        match existing {
            Some(note) if now.saturating_sub(note.first_seen) >= grace_secs => {
                to_delete.push(key.clone());
                if to_delete.len() >= PAGE_SIZE {
                    outcome.deleted += flush_deletes(dest, dest_cipher, &mut to_delete).await?;
                }
            }
            // Noted, but not old enough yet: left exactly as it is.
            Some(_) => {}
            None => {
                let note = Orphan {
                    key_hash: name.clone(),
                    key: key.clone(),
                    first_seen: now,
                };
                Orphan::COLLECTION
                    .put(dest.as_ref(), dest_cipher, &name, &note, None)
                    .await?;
                outcome.noted += 1;
            }
        }
    }
    outcome.deleted += flush_deletes(dest, dest_cipher, &mut to_delete).await?;

    // Notes whose key is back at the source: dropped, so the grace period
    // measures continuous absence rather than "was missing once".
    //
    // `list_names` rather than a raw prefix walk: it filters out the
    // collection's own `.deleted/`/`.history/` bookkeeping, which a raw walk
    // would hand back as entity names that can't even be built into keys.
    // Materializing the names is fine here in a way it isn't for the bucket
    // walks above — the ledger only ever holds one entry per *currently*
    // orphaned key.
    for name in Orphan::COLLECTION.list_names(dest.as_ref()).await? {
        let (note, _etag) = Orphan::COLLECTION
            .get::<Orphan>(dest.as_ref(), dest_cipher, &name)
            .await?;
        let Some(note) = note else { continue };
        let (_, src_etag) = src.get(&note.key).await?;
        if src_etag.is_some() {
            forget(dest, dest_cipher, &name).await?;
            outcome.cleared += 1;
        }
    }
    Ok(outcome)
}

/// Deletes a batch of destination keys plus their ledger notes, and clears
/// `batch`. `delete_many` chunks to each driver's own limit internally.
async fn flush_deletes(dest: &Arc<dyn Backend>, dest_cipher: &Cipher, batch: &mut Vec<String>) -> Result<u64> {
    if batch.is_empty() {
        return Ok(0);
    }
    let count = batch.len() as u64;
    dest.delete_many(batch).await.context("deleting orphaned objects")?;
    for key in batch.iter() {
        // The note has done its job; a leftover note for a key that's gone
        // would just make the next pass re-derive the same answer.
        forget(dest, dest_cipher, &key_hash(key)).await?;
    }
    batch.clear();
    Ok(count)
}

/// Removes a ledger note completely — the tombstone `Collection::delete`
/// leaves behind included. The ledger is bookkeeping about the destination,
/// not data anyone recovers, and a tombstone per orphan ever seen would
/// accumulate forever with nothing to read it.
async fn forget(dest: &Arc<dyn Backend>, dest_cipher: &Cipher, name: &str) -> Result<()> {
    Orphan::COLLECTION.delete(dest.as_ref(), dest_cipher, name).await?;
    Orphan::COLLECTION
        .forget_tombstone(dest.as_ref(), dest_cipher, name)
        .await?;
    Ok(())
}

/// Deletes every snapshot past the newest `keep`. `keep` of 0 is treated as
/// "keep one": a retention setting should never be able to leave an instance
/// with no snapshot at all in one pass.
pub async fn prune_snapshots(dest: &Arc<dyn Backend>, dest_cipher: &Cipher, keep: u32) -> Result<u64> {
    let keep = keep.max(1) as usize;
    let manifests = snapshot::list(dest, dest_cipher).await?;
    if manifests.len() <= keep {
        return Ok(0);
    }
    let mut deleted = 0;
    for manifest in &manifests[keep..] {
        snapshot::delete(dest, &manifest.id)
            .await
            .with_context(|| format!("deleting snapshot {}", manifest.id))?;
        deleted += 1;
    }
    Ok(deleted)
}

/// Deletes whole `.conf/audit/<YYYY-MM>/` months past the newest
/// `keep_months` at the destination.
///
/// The audit trail is the one class that grows without bound, which is why
/// it gets a retention knob the other classes don't. Month granularity keeps
/// this a prefix delete rather than a per-entry scan, and it never touches
/// the source's own audit trail — only the destination's copy of it.
pub async fn prune_audit_months(dest: &Arc<dyn Backend>, keep_months: u32) -> Result<u64> {
    let keep = keep_months.max(1) as usize;
    let root = format!("{}/", crate::storage::audit::AUDIT_ROOT_PUBLIC);
    let mut months: Vec<String> = Vec::new();
    let mut pages = KeyPages::new(Arc::clone(dest), &root);
    while let Some((key, _size)) = pages.next().await? {
        let Some(month) = key.strip_prefix(&root).and_then(|rest| rest.split('/').next()) else {
            continue;
        };
        // Month directories are `YYYY-MM`; a flat legacy entry directly under
        // the root (written before month bucketing existed) has no month
        // segment and is left alone rather than guessed at.
        if month.len() == 7 && !months.iter().any(|m| m == month) {
            months.push(month.to_string());
        }
    }
    if months.len() <= keep {
        return Ok(0);
    }
    months.sort_unstable_by(|a, b| b.cmp(a));

    let mut deleted = 0;
    for month in &months[keep..] {
        let prefix = format!("{root}{month}/");
        let mut batch: Vec<String> = Vec::new();
        let mut pages = KeyPages::new(Arc::clone(dest), &prefix);
        loop {
            match pages.next().await? {
                Some((key, _size)) => {
                    batch.push(key);
                    if batch.len() < PAGE_SIZE {
                        continue;
                    }
                }
                None if batch.is_empty() => break,
                None => {}
            }
            dest.delete_many(&std::mem::take(&mut batch))
                .await
                .with_context(|| format!("deleting audit month {month}"))?;
        }
        deleted += 1;
    }
    Ok(deleted)
}
