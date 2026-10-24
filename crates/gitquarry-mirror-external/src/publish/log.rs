//! What a publish mirror last published and the record of its recent runs
//! (`docs/publish-mirror-concept.md` §6, "the audit record"): one object per
//! mirror under `repos/<uid>/publish-log/<mirror_id>`.
//!
//! Kept apart from the [`super::PublishMirror`] entity on purpose: a run
//! writes here on every sweep, and writing to the entity instead would make
//! every open edit form lose its CAS race against the sweep. A sibling
//! prefix rather than one nested under `publish-mirrors/<id>/`, because
//! listing that collection would read every nested key as another mirror.

use super::PublishMode;
use super::audit::{Finding, Gate, GateReport};
use crate::access::{RefSyncStatus, cap_error};
use crate::git_remote::redact_url;
use anyhow::Result;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::REPOS_PREFIX;
use gitquarry_core::storage::uid::Uid;
use s3db::{Collection, Update};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Runs kept per mirror, newest first. The sweep runs every 15 minutes, so
/// an unbounded list would grow by a hundred records a day for a mirror
/// with nothing to publish.
pub const MAX_RUNS: usize = 20;

/// Findings kept per gate in a run record. The record is for reading what
/// blocked a run, and a thousand secret-scan hits say no more than twenty.
const MAX_FINDINGS_PER_GATE: usize = 20;

/// A snapshot commit this mirror pushed to a public branch, and its tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicHead {
    pub commit: String,
    pub tree: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishLog {
    /// Public ref -> the snapshot this mirror last pushed there. The next
    /// run builds on it, and only on it: a public branch found anywhere
    /// else was moved by somebody else.
    #[serde(default)]
    pub heads: BTreeMap<String, PublicHead>,
    /// Public ref -> the snapshot about to be pushed, written before the
    /// push. A run that pushed and then failed to record it would otherwise
    /// find its own commit on the public side and refuse it as foreign.
    #[serde(default)]
    pub pending: BTreeMap<String, PublicHead>,
    #[serde(default)]
    pub ref_status: BTreeMap<String, RefSyncStatus>,
    /// Why the last run failed or was blocked, `None` after one that wasn't.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Newest first, at most [`MAX_RUNS`].
    #[serde(default)]
    pub runs: Vec<PublishRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RunOutcome {
    Published,
    /// Every published tree already matched the public side: nothing pushed.
    Unchanged,
    /// A gate refused; nothing was pushed.
    Blocked,
    /// The public branch holds commits this mirror didn't publish.
    Diverged,
    Failed,
}

/// One gate's verdict as stored, its findings capped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRecord {
    pub gate: Gate,
    pub findings: Vec<Finding>,
    /// Findings past the cap, counted but not kept.
    #[serde(default)]
    pub omitted: usize,
}

impl GateRecord {
    pub fn passed(&self) -> bool {
        self.findings.is_empty() && self.omitted == 0
    }
}

impl From<GateReport> for GateRecord {
    fn from(report: GateReport) -> Self {
        let omitted = report.findings.len().saturating_sub(MAX_FINDINGS_PER_GATE);
        let mut findings = report.findings;
        findings.truncate(MAX_FINDINGS_PER_GATE);
        Self {
            gate: report.gate,
            findings,
            omitted,
        }
    }
}

/// One published branch within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefRun {
    pub ref_name: String,
    pub private_head: String,
    /// What the public branch held when the run started.
    pub previous_public_head: Option<String>,
    /// The snapshot that holds this ref's tree after the run: the commit
    /// pushed, or the previous one when nothing changed. `None` when the run
    /// stopped before building one.
    pub public_head: Option<String>,
    /// The tree of [`Self::public_head`].
    #[serde(default)]
    pub public_tree: Option<String>,
    /// Files and gitlinks in the published tree.
    pub published_files: usize,
    /// Entries of the private head tree the filter left out; an excluded
    /// directory counts once.
    pub excluded_entries: usize,
    pub gates: Vec<GateRecord>,
    pub outcome: RunOutcome,
    /// Why this ref couldn't be published, when it was this ref's fault.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRun {
    pub started_at: String,
    pub duration_ms: u64,
    pub mode: PublishMode,
    /// `None` when the run stopped before reaching the host gate.
    pub host: Option<GateRecord>,
    pub refs: Vec<RefRun>,
    pub outcome: RunOutcome,
    /// Redacted and capped like [`crate::access::ExternalMirror::last_error`].
    pub error: Option<String>,
}

impl PublishRun {
    pub fn gates_passed(&self) -> bool {
        self.host.as_ref().is_some_and(GateRecord::passed)
            && self.refs.iter().all(|r| r.gates.iter().all(GateRecord::passed))
    }
}

/// The same redaction and cap every stored mirror error goes through.
pub(crate) fn stored_error(text: &str) -> String {
    cap_error(&redact_url(text))
}

fn log_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/publish-log"))
}

/// The mirror's log, empty for one that has never run.
pub async fn get(ctx: &AppContext, uid: &Uid, mirror_id: &str) -> Result<PublishLog> {
    let (log, _etag) = log_collection(uid)
        .get::<PublishLog>(ctx.backend().as_ref(), &ctx.cipher, mirror_id)
        .await?;
    Ok(log.unwrap_or_default())
}

/// Read-modify-write under s3db's own CAS retry loop: `apply` may run more
/// than once and must derive everything from the log it is handed.
pub(crate) async fn update(
    ctx: &AppContext,
    uid: &Uid,
    mirror_id: &str,
    mut apply: impl FnMut(&mut PublishLog),
) -> Result<()> {
    log_collection(uid)
        .update::<PublishLog, _>(ctx.backend().as_ref(), &ctx.cipher, mirror_id, |log| {
            apply(log);
            Ok(Update::Write)
        })
        .await?;
    Ok(())
}

pub(crate) async fn delete(ctx: &AppContext, uid: &Uid, mirror_id: &str) -> Result<()> {
    log_collection(uid)
        .delete(ctx.backend().as_ref(), &ctx.cipher, mirror_id)
        .await
}

impl PublishLog {
    pub(crate) fn push_run(&mut self, run: PublishRun) {
        self.runs.insert(0, run);
        self.runs.truncate(MAX_RUNS);
    }
}
