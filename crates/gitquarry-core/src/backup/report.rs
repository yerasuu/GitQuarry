//! What a backup run recorded about itself, stored bucket-side at
//! `.conf/backup-runs/<millis>-<rand>.json`.
//!
//! Flat, not month-bucketed like [`crate::storage::audit`]: an instance
//! takes a handful of backups a day at most, so the whole collection stays
//! small enough for one LIST plus a sort, and the extra prefix segment
//! audit needs to avoid listing its entire history would buy nothing here.
//!
//! A run writes its record **twice** — once at the start (`Running`) and
//! once at the end. That's deliberate: a run that dies halfway (the process
//! is killed, the destination goes away) then leaves a visible `Running`
//! record with a start time, instead of leaving no trace at all and making
//! a crashed backup indistinguishable from a backup that was never
//! scheduled.

use crate::backup::inventory::Class;
use crate::crypto::Cipher;
use crate::storage::{Backend, Cas};
use anyhow::Result;
use s3db::Entity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// How many run records `/_settings/backup` lists, and the cap
/// [`prune_old_runs`] trims back to. A report is for answering "did last
/// night's backup work", not for long-term history — that's what the
/// destination itself is.
pub const LIST_LIMIT: usize = 20;

/// Cap on the recorded failure and unknown-key samples. A run against a
/// broken destination fails on *every* key; recording all of them would
/// turn one bad night into a multi-megabyte report object (and the s3db
/// object-size guard would then refuse to write it at all, losing the
/// report entirely — the failure mode this cap exists to prevent).
pub const MAX_SAMPLES: usize = 20;

/// What started a run. Kept because "the schedule fired" and "an admin hit
/// the button" have different follow-up: a missing scheduled run is a
/// broken instance, a missing manual one is just a button nobody pressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    #[default]
    Scheduled,
    /// The settings page's "run now".
    Manual,
    /// `gitquarry backup` on the command line.
    Cli,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Written at the start of every run — see this module's doc comment.
    #[default]
    Running,
    /// Finished with nothing failed.
    Ok,
    /// Finished, but at least one object failed to copy. Still a completed
    /// run: the counters below say how much did land, and a partial backup
    /// is worth knowing about rather than discarding.
    Partial,
    /// Didn't get far enough to copy anything meaningful — an unresolvable
    /// destination, a passphrase mismatch, a listing that failed outright.
    Failed,
}

/// Per-class counters. Bytes are the *stored* (sealed) sizes the backend
/// reports, not plaintext sizes: that's what the destination will actually
/// hold, and it's what a `list_prefix` already returns without an extra
/// read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassStat {
    #[serde(default)]
    pub copied: u64,
    /// Already present at the destination with the same size — the whole
    /// point of an incremental run.
    #[serde(default)]
    pub skipped: u64,
    #[serde(default)]
    pub failed: u64,
    #[serde(default)]
    pub bytes_copied: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Entity)]
#[entity(collection = ".conf/backup-runs")]
pub struct BackupRun {
    /// `<zero-padded millis>-<random>` — lexicographic order is
    /// chronological order, same scheme (and same reasoning) as
    /// `storage::audit::entry_name`, so "list, sort, take the tail" needs
    /// no bodies read.
    #[serde(default)]
    #[entity(key)]
    pub id: String,
    #[serde(default)]
    pub started_at: u64,
    /// `None` while the run is still `Outcome::Running`.
    #[serde(default)]
    pub finished_at: Option<u64>,
    /// The destination string as configured (`bucket:name`, `path:/dir`,
    /// `self`) — never credentials, which live only in the local
    /// `gitquarry.toml`. Recorded so a report still says where it went
    /// after someone edits the setting.
    #[serde(default)]
    pub dest: String,
    #[serde(default)]
    pub trigger: Trigger,
    #[serde(default)]
    pub outcome: Outcome,
    /// Keyed by [`Class::label`], which is why that label must stay stable.
    #[serde(default)]
    pub classes: BTreeMap<String, ClassStat>,
    /// Sample of keys that matched no rule in
    /// [`crate::backup::inventory::classify`] — they were copied anyway, and
    /// this is the operator's cue to go classify them. Capped at
    /// [`MAX_SAMPLES`].
    #[serde(default)]
    pub unknown_keys: Vec<String>,
    /// Sample of `key: error` lines, capped at [`MAX_SAMPLES`].
    #[serde(default)]
    pub failures: Vec<String>,
    /// How many failures there were in total, which
    /// [`Self::failures`]' cap otherwise hides.
    #[serde(default)]
    pub failures_total: u64,
    /// The `backup_id` of the snapshot half of the run, when one ran.
    #[serde(default)]
    pub snapshot_id: Option<String>,
    /// Set only when the run ended `Failed` before any copying — the reason
    /// it never started, which no per-key failure line would carry.
    #[serde(default)]
    pub error: Option<String>,
}

impl BackupRun {
    /// A fresh `Running` record for a run starting now.
    pub fn starting(dest: &str, trigger: Trigger) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        BackupRun {
            id: run_id(now.as_millis()),
            started_at: now.as_secs(),
            finished_at: None,
            dest: dest.to_string(),
            trigger,
            outcome: Outcome::Running,
            ..Default::default()
        }
    }

    /// Adds one object's result to its class's counters, creating the entry
    /// on first use so a class that saw no objects has no row at all rather
    /// than a row of zeros.
    pub fn count(&mut self, class: Class, copied_bytes: Option<u64>) {
        let stat = self.classes.entry(class.label().to_string()).or_default();
        match copied_bytes {
            Some(bytes) => {
                stat.copied += 1;
                stat.bytes_copied += bytes;
            }
            None => stat.skipped += 1,
        }
    }

    /// Records a per-object failure. Counts every one, keeps
    /// [`MAX_SAMPLES`] of them.
    pub fn count_failure(&mut self, class: Class, key: &str, error: &str) {
        self.classes.entry(class.label().to_string()).or_default().failed += 1;
        self.failures_total += 1;
        if self.failures.len() < MAX_SAMPLES {
            self.failures.push(format!("{key}: {error}"));
        }
    }

    /// Notes a key nothing classified. Copied regardless — this is the
    /// paper trail, not a gate.
    pub fn note_unknown(&mut self, key: &str) {
        if self.unknown_keys.len() < MAX_SAMPLES {
            self.unknown_keys.push(key.to_string());
        }
    }

    /// Stamps the end of the run. `Partial` rather than `Ok` whenever
    /// anything failed at all: a report that says `Ok` has to mean every
    /// selected object is at the destination.
    pub fn finish(&mut self) {
        self.finished_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        self.outcome = if self.failures_total > 0 {
            Outcome::Partial
        } else {
            Outcome::Ok
        };
    }

    /// Stamps the end of a run that never got going. `error` is the reason
    /// no per-key failure line can explain.
    pub fn fail(&mut self, error: String) {
        self.finished_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        self.outcome = Outcome::Failed;
        self.error = Some(error);
    }

    pub fn duration_secs(&self) -> Option<u64> {
        self.finished_at.map(|end| end.saturating_sub(self.started_at))
    }

    /// Totals across every class, for the one-line summary the settings
    /// page shows per run.
    pub fn totals(&self) -> ClassStat {
        let mut total = ClassStat::default();
        for stat in self.classes.values() {
            total.copied += stat.copied;
            total.skipped += stat.skipped;
            total.failed += stat.failed;
            total.bytes_copied += stat.bytes_copied;
        }
        total
    }
}

/// Same zero-padded-millis-then-random scheme as
/// `storage::audit::entry_name` — see [`BackupRun::id`].
fn run_id(at_millis: u128) -> String {
    format!("{at_millis:020}-{:08x}", rand::random::<u32>())
}

/// Writes (or overwrites) `run`'s record. Not CAS-guarded on purpose: a run
/// record has exactly one writer, the run itself, from start to finish —
/// there's nothing to race with, and a lost `finish()` write would leave a
/// permanently-`Running` record for no reason.
pub async fn save(backend: &dyn Backend, cipher: &Cipher, run: &BackupRun) -> Result<()> {
    let key = BackupRun::COLLECTION.key(&run.id)?;
    let (_, etag) = backend.get(&key).await?;
    let cas = BackupRun::COLLECTION
        .put(backend, cipher, &run.id, run, etag.as_deref())
        .await?;
    if cas == Cas::Conflict {
        // Two writers on one run id shouldn't be possible; say so rather
        // than silently dropping the record.
        anyhow::bail!("backup run {} was written concurrently by something else", run.id);
    }
    Ok(())
}

/// The most recent runs, newest first, at most `limit` of them.
pub async fn list_recent(backend: &dyn Backend, cipher: &Cipher, limit: usize) -> Result<Vec<BackupRun>> {
    let mut names = BackupRun::COLLECTION.list_names(backend).await?;
    names.sort_unstable_by(|a, b| b.cmp(a));
    names.truncate(limit);
    let mut runs = Vec::with_capacity(names.len());
    for name in names {
        // A record that fails to decode is skipped, not fatal: the point of
        // this list is showing the operator the runs that *are* readable,
        // and one unreadable record must not hide the other nineteen.
        if let Ok((Some(run), _etag)) = BackupRun::COLLECTION.get::<BackupRun>(backend, cipher, &name).await {
            runs.push(run);
        }
    }
    Ok(runs)
}

/// Deletes every run record beyond the newest [`LIST_LIMIT`]. Called at the
/// end of each run so the collection can't grow without bound — unlike the
/// destination's own retention (which deletes real backed-up data and is
/// therefore opt-in and grace-guarded), dropping an old *report* loses
/// nothing recoverable, so it needs no operator consent.
pub async fn prune_old_runs(backend: &dyn Backend, cipher: &Cipher) -> Result<usize> {
    let mut names = BackupRun::COLLECTION.list_names(backend).await?;
    if names.len() <= LIST_LIMIT {
        return Ok(0);
    }
    names.sort_unstable_by(|a, b| b.cmp(a));
    let stale = &names[LIST_LIMIT..];
    for name in stale {
        BackupRun::COLLECTION.delete(backend, cipher, name).await?;
    }
    Ok(stale.len())
}
