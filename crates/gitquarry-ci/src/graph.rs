//! Ingests a `JobGraph` dumped by an SDK (`gitquarry-ci-sdk` or
//! `gitquarry_ci`), validates it, and decides each job's initial run
//! status — this is where the deploy-approval gate lives
//! (`docs/gitquarry-ci-sdk-design.md`). No execution engine yet
//! (`docs/ci-cd-concept.md` Phase 4 isn't built) — a `Queued` run just sits
//! in storage until that lands; this module only owns "should this job be
//! allowed to start at all yet".

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use gitquarry_graph_validate::{DEFAULT_MAX_JOBS, JobNode, validate_graph};
use gitquarry_wire::{JobGraph, JobKind};
use s3db::{Backend, Cas, Cipher, Collection};
use serde::{Deserialize, Serialize};

use crate::core_client::CoreClient;

const CI_PREFIX: &str = "ci";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum JobRunStatus {
    /// A `kind: deploy` job on a repo that hasn't opted out of the gate —
    /// won't run until a maintainer approves it.
    PendingApproval,
    #[default]
    Queued,
    Approved,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobRun {
    pub id: String,
    pub repo_uid: String,
    pub sha: String,
    pub job_id: String,
    pub kind: JobKind,
    pub status: JobRunStatus,
    pub created_at: u64,
}

fn runs_collection(repo_uid: &str) -> Collection {
    Collection::new_owned(format!("{CI_PREFIX}/{repo_uid}/runs"))
}

/// Validates `graph`, looks up the repo's gate opt-out, and stores one
/// `JobRun` per job — `PendingApproval` for a `deploy` job unless the repo
/// disabled the gate, `Queued` otherwise.
pub async fn ingest_graph(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    core: &CoreClient,
    repo_uid: &str,
    sha: &str,
    graph: JobGraph,
) -> Result<Vec<JobRun>> {
    let nodes: Vec<JobNode> = graph
        .jobs
        .iter()
        .map(|j| JobNode {
            id: &j.id,
            image: &j.image,
            needs: &j.needs,
        })
        .collect();
    if let Err(errors) = validate_graph(&nodes, DEFAULT_MAX_JOBS) {
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        bail!("invalid job graph: {}", messages.join("; "));
    }

    let repo = core
        .repo_info(repo_uid)
        .await?
        .with_context(|| format!("gitquarry-core has no repo with uid {repo_uid}"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let collection = runs_collection(repo_uid);
    let mut runs = Vec::with_capacity(graph.jobs.len());

    for job in graph.jobs {
        let status = if job.kind == JobKind::Deploy && !repo.ci_deploy_gate_disabled {
            JobRunStatus::PendingApproval
        } else {
            JobRunStatus::Queued
        };
        let run = JobRun {
            id: format!("{sha}-{}", job.id),
            repo_uid: repo_uid.to_string(),
            sha: sha.to_string(),
            job_id: job.id.clone(),
            kind: job.kind,
            status,
            created_at: now,
        };
        collection
            .put(backend.as_ref(), cipher.as_ref(), &run.id, &run, None)
            .await
            .with_context(|| format!("storing run {}", run.id))?;
        runs.push(run);
    }

    Ok(runs)
}

pub async fn list_pending_approvals(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    repo_uid: &str,
) -> Result<Vec<JobRun>> {
    runs_collection(repo_uid)
        .find_all(backend, cipher, |run: &JobRun| {
            run.status == JobRunStatus::PendingApproval
        })
        .await
}

/// `Ok(false)` if no such run exists; `Err` if it exists but isn't
/// `PendingApproval` (nothing to approve) or a concurrent edit lost the
/// CAS race.
pub async fn approve_run(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    repo_uid: &str,
    run_id: &str,
) -> Result<bool> {
    let collection = runs_collection(repo_uid);
    let (run, etag) = collection
        .get::<JobRun>(backend.as_ref(), cipher.as_ref(), run_id)
        .await?;
    let Some(mut run) = run else {
        return Ok(false);
    };
    if run.status != JobRunStatus::PendingApproval {
        bail!("run {run_id} is not pending approval");
    }
    run.status = JobRunStatus::Approved;
    match collection
        .put(backend.as_ref(), cipher.as_ref(), run_id, &run, etag.as_deref())
        .await?
    {
        Cas::Ok => Ok(true),
        Cas::Conflict => bail!("run {run_id} was concurrently modified — retry"),
    }
}

#[cfg(test)]
mod tests;
