//! The GC sweep of abandoned publish epochs (`docs/publish-mirror-concept.md`
//! §3, "Epochs": an old epoch's mapping and publish repo are kept "until the
//! next GC sweep, for rollback"). [`plan`] reads and decides, [`apply`]
//! deletes what a plan listed as removable and nothing else; the operator
//! command around them is [`command`].
//!
//! The rules, strongest first:
//! - A mirror's current epochs are never touched: its `filter_epoch` and
//!   the epoch its run log last published, both when they differ (a bump
//!   not yet published), and anything above `filter_epoch`.
//! - A mirror whose entity or run log doesn't read is never touched, nor
//!   is an epoch whose mirror entity is gone: the listing carries no object
//!   ages, so an orphan has no moment its grace could count from and is
//!   only reported.
//! - Any other epoch goes once its grace has run from the moment recorded
//!   in [`PublishLog::abandoned`]. One with no record gets one, now, even
//!   from a dry run, and is never deleted by the sweep that recorded it.
//!   A soft-deleted repo's mirrors follow the same rule: their entities
//!   are still there.
//!
//! Packs piling up inside the current epoch (one per run) are counted and
//! never consolidated here.

pub mod command;
mod keys;

use super::epoch::published_epoch;
use super::log::{PublishLog, log_collection, unix_now};
use super::{PublishMirror, publish_mirrors_collection};
use anyhow::{Context, Result, bail};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{Backend, REPOS_PREFIX};
use keys::EpochKeys;
use std::collections::BTreeMap;
use tracing::warn;

const DAY_SECS: u64 = 86_400;

/// The default grace, in days, between an epoch's abandonment and the
/// first sweep allowed to delete it.
pub const DEFAULT_GRACE_DAYS: u32 = 30;

#[derive(Debug)]
pub struct GcReport {
    pub grace_days: u32,
    pub mirrors: Vec<MirrorGc>,
    /// Keys under a `publish/` prefix that are no epoch object of a valid
    /// repo uid, mirror id and epoch. Reported, never deleted.
    pub unrecognized: Vec<String>,
}

impl GcReport {
    pub fn epochs(&self) -> impl Iterator<Item = (&MirrorGc, &EpochGc)> {
        self.mirrors
            .iter()
            .flat_map(|mirror| mirror.epochs.iter().map(move |epoch| (mirror, epoch)))
    }
}

#[derive(Debug)]
pub struct MirrorGc {
    pub repo: Uid,
    pub mirror_id: String,
    pub status: MirrorStatus,
    pub epochs: Vec<EpochGc>,
}

#[derive(Debug, PartialEq)]
pub enum MirrorStatus {
    Live {
        filter_epoch: u32,
        published: Option<u32>,
    },
    /// The entity or the run log didn't read: the error, redacted.
    Unreadable(String),
    /// No mirror entity for this id any more.
    Orphan,
}

#[derive(Debug)]
pub struct EpochGc {
    pub epoch: u32,
    pub objects: usize,
    pub bytes: u64,
    pub packs: usize,
    pub pack_bytes: u64,
    pub verdict: Verdict,
    keys: EpochKeys,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// The mirror's epoch or its published one.
    Current,
    /// Above the mirror's `filter_epoch`: nothing abandoned it.
    Ahead,
    /// The mirror is unreadable or gone.
    Untouched,
    /// Abandoned at `abandoned_at`, removable from `eligible_at` (Unix
    /// seconds). `recorded_now` when this sweep started the clock.
    InGrace {
        abandoned_at: u64,
        eligible_at: u64,
        recorded_now: bool,
    },
    Removable,
    Deleted,
    /// What stopped a delete or a record; the next sweep retries.
    Failed(String),
}

/// Reads every publish epoch under `repo` (or the whole bucket) and decides
/// each one. Deletes nothing; the only write is the abandonment moment of
/// an abandoned epoch that has none, into its mirror's run log.
///
/// # Errors
///
/// Only when the bucket can't be listed. A mirror that can't be read, or
/// whose record can't be written, is reported in its [`MirrorGc`] instead.
pub async fn plan(backend: &dyn Backend, cipher: &Cipher, repo: Option<&Uid>, grace_days: u32) -> Result<GcReport> {
    let prefix = match repo {
        Some(uid) => format!("{REPOS_PREFIX}/{uid}/publish/"),
        None => format!("{REPOS_PREFIX}/"),
    };
    let listing = keys::classify(backend.list_prefix(&prefix).await?);
    let now = unix_now();
    let grace = u64::from(grace_days) * DAY_SECS;
    let mut mirrors = Vec::with_capacity(listing.mirrors.len());
    for ((uid, mirror_id), epochs) in listing.mirrors {
        let mut epochs: Vec<_> = epochs
            .into_iter()
            .map(|(epoch, keys)| EpochGc {
                epoch,
                objects: keys.objects(),
                bytes: keys.bytes,
                packs: keys.packs.len(),
                pack_bytes: keys.pack_bytes,
                verdict: Verdict::Untouched,
                keys,
            })
            .collect();
        let status = match read_state(backend, cipher, &uid, &mirror_id).await {
            Ok(Some(state)) => {
                decide(&state, &mut epochs, now, grace);
                record_missing(backend, cipher, &uid, &mirror_id, &mut epochs, now).await;
                state.status()
            }
            Ok(None) => MirrorStatus::Orphan,
            Err(e) => MirrorStatus::Unreadable(super::log::stored_error(&format!("{e:#}"))),
        };
        mirrors.push(MirrorGc {
            repo: uid,
            mirror_id,
            status,
            epochs,
        });
    }
    Ok(GcReport {
        grace_days,
        mirrors,
        unrecognized: listing.unrecognized,
    })
}

/// Deletes every epoch `report` lists as [`Verdict::Removable`], after
/// reading its mirror again and deciding it again: an epoch that became
/// current, lost its record or isn't past its grace any more since the
/// plan is left alone. Each verdict becomes `Deleted`, `Failed` or what
/// the second look decided. Never deletes a key the plan didn't list.
pub async fn apply(backend: &dyn Backend, cipher: &Cipher, report: &mut GcReport) {
    let grace = u64::from(report.grace_days) * DAY_SECS;
    for mirror in &mut report.mirrors {
        if !mirror.epochs.iter().any(|e| e.verdict == Verdict::Removable) {
            continue;
        }
        let state = match read_state(backend, cipher, &mirror.repo, &mirror.mirror_id).await {
            Ok(Some(state)) => state,
            Ok(None) => {
                mark_removable(mirror, Verdict::Untouched);
                mirror.status = MirrorStatus::Orphan;
                continue;
            }
            Err(e) => {
                mark_removable(
                    mirror,
                    Verdict::Failed(format!("couldn't read the mirror again: {e:#}")),
                );
                continue;
            }
        };
        let now = unix_now();
        let mut deleted = Vec::new();
        for epoch in mirror.epochs.iter_mut().filter(|e| e.verdict == Verdict::Removable) {
            epoch.verdict = state.verdict(epoch.epoch, now, grace);
            if epoch.verdict != Verdict::Removable {
                continue;
            }
            epoch.verdict = match delete_epoch(backend, &epoch.keys).await {
                Ok(()) => {
                    deleted.push(epoch.epoch);
                    Verdict::Deleted
                }
                Err(e) => Verdict::Failed(format!("{e:#}")),
            };
        }
        if !deleted.is_empty()
            && let Err(e) = update_log(backend, cipher, &mirror.repo, &mirror.mirror_id, |log| {
                for epoch in &deleted {
                    log.abandoned.remove(epoch);
                }
            })
            .await
        {
            // Harmless: a record for an epoch with no objects left is
            // never looked at again.
            warn!(
                target: "publish::gc",
                repo = %mirror.repo,
                mirror = %mirror.mirror_id,
                error = %format!("{e:#}"),
                "couldn't drop the records of the deleted epochs"
            );
        }
        mirror.status = state.status();
    }
}

fn mark_removable(mirror: &mut MirrorGc, verdict: Verdict) {
    for epoch in mirror.epochs.iter_mut().filter(|e| e.verdict == Verdict::Removable) {
        epoch.verdict = verdict.clone();
    }
}

struct State {
    filter_epoch: u32,
    published: Option<u32>,
    abandoned: BTreeMap<u32, u64>,
}

impl State {
    fn status(&self) -> MirrorStatus {
        MirrorStatus::Live {
            filter_epoch: self.filter_epoch,
            published: self.published,
        }
    }

    /// `InGrace` with `recorded_now` for an abandoned epoch with no record.
    fn verdict(&self, epoch: u32, now: u64, grace: u64) -> Verdict {
        if epoch == self.filter_epoch || Some(epoch) == self.published {
            return Verdict::Current;
        }
        if epoch > self.filter_epoch {
            return Verdict::Ahead;
        }
        match self.abandoned.get(&epoch) {
            Some(&at) if now >= at.saturating_add(grace) => Verdict::Removable,
            Some(&at) => Verdict::InGrace {
                abandoned_at: at,
                eligible_at: at.saturating_add(grace),
                recorded_now: false,
            },
            None => Verdict::InGrace {
                abandoned_at: now,
                eligible_at: now.saturating_add(grace),
                recorded_now: true,
            },
        }
    }
}

fn decide(state: &State, epochs: &mut [EpochGc], now: u64, grace: u64) {
    for epoch in epochs {
        epoch.verdict = state.verdict(epoch.epoch, now, grace);
    }
}

/// `Ok(None)` when the mirror entity doesn't exist.
async fn read_state(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, mirror_id: &str) -> Result<Option<State>> {
    let (mirror, _etag) = publish_mirrors_collection(uid)
        .get::<PublishMirror>(backend, cipher, mirror_id)
        .await
        .context("reading the mirror entity")?;
    let Some(mirror) = mirror else {
        return Ok(None);
    };
    let (log, _etag) = log_collection(uid)
        .get::<PublishLog>(backend, cipher, mirror_id)
        .await
        .context("reading the run log")?;
    let log = log.unwrap_or_default();
    Ok(Some(State {
        filter_epoch: mirror.filter_epoch,
        published: published_epoch(&log),
        abandoned: log.abandoned,
    }))
}

async fn record_missing(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    epochs: &mut [EpochGc],
    now: u64,
) {
    let missing: Vec<u32> = epochs
        .iter()
        .filter(|e| matches!(e.verdict, Verdict::InGrace { recorded_now: true, .. }))
        .map(|e| e.epoch)
        .collect();
    if missing.is_empty() {
        return;
    }
    let recorded = update_log(backend, cipher, uid, mirror_id, |log| {
        for epoch in &missing {
            log.abandoned.entry(*epoch).or_insert(now);
        }
    })
    .await;
    if let Err(e) = recorded {
        // Without a stored moment the grace never starts, which keeps the
        // epoch; saying so is what tells the operator why it never goes.
        for epoch in epochs.iter_mut().filter(|e| missing.contains(&e.epoch)) {
            epoch.verdict = Verdict::Failed(format!("couldn't record when it was abandoned: {e:#}"));
        }
    }
}

async fn update_log(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    mut apply: impl FnMut(&mut PublishLog),
) -> Result<()> {
    log_collection(uid)
        .update::<PublishLog, _>(backend, cipher, mirror_id, |log| {
            apply(log);
            Ok(s3db::Update::Write)
        })
        .await?;
    Ok(())
}

async fn delete_epoch(backend: &dyn Backend, keys: &EpochKeys) -> Result<()> {
    for (part, batch) in keys.batches() {
        if batch.is_empty() {
            continue;
        }
        let outcome = backend
            .delete_many(batch)
            .await
            .with_context(|| format!("deleting the {part} objects"))?;
        if outcome.confirmed < outcome.attempted {
            bail!(
                "{} of {} {part} object(s) not confirmed deleted; stopped there so the rest of the epoch stays \
                 for the next sweep",
                outcome.attempted - outcome.confirmed,
                outcome.attempted
            );
        }
    }
    Ok(())
}
