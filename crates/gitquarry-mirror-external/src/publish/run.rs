//! One publish run (`docs/publish-mirror-concept.md` §4–§5): host gate →
//! build and gate every branch locally ([`super::snapshot`]) → push → record.
//!
//! The order is the contract. Every gate of every branch has run, on the
//! objects actually written, before anything leaves this machine; one
//! blocked branch blocks the whole run, and the push of all branches is one
//! atomic `git push`, so a run publishes everything or nothing. The public
//! branch only ever moves by fast-forward: a snapshot's parent is the public
//! head as the run found it, and nothing here ever forces.
//!
//! Direction is push-only by construction: nothing in this module writes
//! into the private repo or reads a public object back into it.

use super::audit::{self, Finding, GateReport};
use super::log::{self, GateRecord, PublicHead, PublishRun, RefRun, RunOutcome, stored_error};
use super::objects::{Oid, to_hex};
use super::snapshot::{self, RefBuild, RefError, Snapshot};
use super::{PublishMirror, PublishMode};
use crate::access::RefSyncStatus;
use crate::git_remote;
use anyhow::{Context, Result, bail};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// The "publish now" button: one mirror, enabled or not, with the error
/// handed back to the click instead of only to the log. Queues behind any
/// sweep of the same repo, the same exclusion as
/// [`crate::engine::sync_one_mirror_now`].
///
/// # Errors
///
/// Whatever stopped the run from publishing: a gate, a public branch moved
/// by somebody else, the remote. The run is recorded either way.
pub async fn publish_mirror_now(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &PublishMirror,
) -> Result<PublishRun> {
    let _sweep_guard = ctx.locks.lock(&crate::engine::sweep_lock_key(&resolved.uid)).await;
    publish_once(ctx, resolved, mirror).await
}

/// The periodic sweep's half: every enabled publish mirror of one repo, one
/// at a time. A mirror failing is recorded on it and logged, never a reason
/// to skip the others.
///
/// # Errors
///
/// Fails only when the repo's config or its mirror list can't be read.
pub async fn publish_repo_mirrors(ctx: &AppContext, resolved: &ResolvedRepo) -> Result<()> {
    let _sweep_guard = ctx.locks.lock(&crate::engine::sweep_lock_key(&resolved.uid)).await;
    // Same rule as the external mirrors' sweep: a repo its owner deleted is
    // not published anywhere any more.
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if config.deleted_at.is_some() {
        return Ok(());
    }
    for (mirror, _etag) in super::list_with_etags(ctx, &resolved.uid).await? {
        if !mirror.enabled {
            continue;
        }
        if let Err(e) = publish_once(ctx, resolved, &mirror).await {
            warn!(
                target: "publish",
                repo = %resolved.uid,
                mirror = %mirror.id,
                error = %stored_error(&format!("{e:#}")),
                "publish run failed"
            );
        }
    }
    Ok(())
}

/// One snapshot about to be pushed.
struct Push {
    ref_name: String,
    commit: Oid,
    parent: Option<Oid>,
    head: PublicHead,
}

/// Runs and records one publish of `mirror`. The caller holds the sweep
/// lock. `mirror` is used as given, never re-read: the same contract as
/// [`crate::engine::sync_mirror_once`].
async fn publish_once(ctx: &AppContext, resolved: &ResolvedRepo, mirror: &PublishMirror) -> Result<PublishRun> {
    let started = Instant::now();
    let mut run = PublishRun {
        started_at: gitquarry_core::storage::audit::format_unix_secs(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        ),
        duration_ms: 0,
        mode: mirror.mode,
        host: None,
        refs: Vec::new(),
        outcome: RunOutcome::Failed,
        error: None,
    };
    let result = execute(ctx, resolved, mirror, &mut run).await;
    run.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    if let Err(e) = &result {
        run.error = Some(stored_error(&format!("{e:#}")));
    }
    settle_ref_outcomes(&mut run);
    info!(
        target: "publish",
        repo = %resolved.uid,
        mirror = %mirror.id,
        outcome = ?run.outcome,
        duration_ms = run.duration_ms,
        "publish run finished"
    );
    let recorded = record(ctx, resolved, mirror, &run).await;
    match (result, recorded) {
        (Err(e), _) => Err(anyhow::anyhow!(
            run.error.unwrap_or_else(|| stored_error(&format!("{e:#}")))
        )),
        (Ok(()), Err(e)) => Err(e.context("the run finished but couldn't be recorded")),
        (Ok(()), Ok(())) => Ok(run),
    }
}

/// The run itself; sets `run`'s outcome, refs and host gate as it goes.
async fn execute(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &PublishMirror,
    run: &mut PublishRun,
) -> Result<()> {
    match mirror.mode {
        PublishMode::Snapshot => {}
    }
    // The publish edge of the host allowlist: narrowing it after a mirror
    // was saved must stop the mirror, not wait for its next edit.
    let host = audit::host_gate(&mirror.remote_url, &super::allowed_hosts(ctx).await?);
    let host_error = host_summary(&host);
    run.host = Some(GateRecord::from(host));
    if let Some(error) = host_error {
        run.outcome = RunOutcome::Blocked;
        bail!("blocked by the host gate, nothing was pushed: {error}");
    }
    if mirror.token.is_empty() {
        bail!("no token configured for the public remote");
    }
    let url = mirror.remote_url.as_str();
    let token = mirror.token.expose();
    // Before taking the repo's lock, like the external mirrors: the round
    // trip most likely to be slow must not hold up clones and pushes.
    let remote_refs = git_remote::ls_remote_refs(url, token).await?;
    let log = log::get(ctx, &resolved.uid, &mirror.id).await?;

    let scratch = gitquarry_core::git::scratch_dir(&std::env::temp_dir())?;
    let scratch_path = scratch.path().to_path_buf();
    let local = ctx.open_mirror_read(resolved).await?;
    let owned_mirror = mirror.clone();
    // The private repo's lock moves in with `local`, so it is held for the
    // local half only and released before anything touches the network.
    let build = tokio::task::spawn_blocking(move || {
        let build = snapshot::build(&local.path, &scratch_path, &owned_mirror, &remote_refs, &log);
        drop(local);
        build
    })
    .await
    .context("the publish task panicked")??;

    run.refs = build.refs.iter().map(ref_run).collect();
    if build.refs.is_empty() {
        bail!("no branch of this repo matches the mirror's published branches");
    }
    if let Some((r, error)) = build.refs.iter().find_map(|r| r.error.as_ref().map(|e| (r, e))) {
        if matches!(error, RefError::Foreign { .. }) {
            run.outcome = RunOutcome::Diverged;
        }
        bail!("{}: {error}", r.ref_name);
    }
    let blocked: Vec<String> = build
        .refs
        .iter()
        .filter(|r| !r.gates_passed())
        .map(|r| format!("{} ({})", r.ref_name, failed_gates(&r.gates)))
        .collect();
    if !blocked.is_empty() {
        run.outcome = RunOutcome::Blocked;
        bail!("blocked by the gates, nothing was pushed: {}", blocked.join("; "));
    }

    let pushes: Vec<Push> = build
        .refs
        .iter()
        .filter_map(|r| match r.snapshot {
            Some(Snapshot::New { commit, tree, parent }) => Some(Push {
                ref_name: r.ref_name.clone(),
                commit,
                parent,
                head: PublicHead {
                    commit: to_hex(&commit),
                    tree: to_hex(&tree),
                },
            }),
            _ => None,
        })
        .collect();
    if pushes.is_empty() {
        run.outcome = RunOutcome::Unchanged;
        return Ok(());
    }
    let verify_path = scratch.path().to_path_buf();
    tokio::task::spawn_blocking(move || snapshot::verify_scratch(&verify_path, &build))
        .await
        .context("the publish task panicked")??;

    log::update(ctx, &resolved.uid, &mirror.id, |log| {
        for push in &pushes {
            log.pending.insert(push.ref_name.clone(), push.head.clone());
        }
    })
    .await
    .context("recording the snapshot about to be pushed")?;

    let scratch_repo = scratch.path().join("public.git");
    let mut refspecs = Vec::with_capacity(pushes.len());
    for push in &pushes {
        // Fetched only now, after every check of the scratch repo: these are
        // public objects, and having the parent locally is what lets `git
        // push` see the new commit as the fast-forward it is.
        if let Some(parent) = &push.parent {
            git_remote::fetch_tip_from_remote_url(&scratch_repo, url, token, &push.ref_name).await?;
            if !has_commit(&scratch_repo, &to_hex(parent)).await? {
                bail!(
                    "{}: the public branch moved while this run was building on {}; nothing was pushed",
                    push.ref_name,
                    to_hex(parent)
                );
            }
        }
        refspecs.push(format!("{}:{}", to_hex(&push.commit), push.ref_name));
    }
    git_remote::push_atomic_to_remote_url(&scratch_repo, url, token, &refspecs).await?;
    run.outcome = RunOutcome::Published;
    Ok(())
}

/// Each branch as planned; [`settle_ref_outcomes`] corrects it once the
/// run's own outcome is known.
fn ref_run(build: &RefBuild) -> RefRun {
    let (outcome, head) = match (&build.error, build.snapshot) {
        (Some(RefError::Foreign { .. }), _) => (RunOutcome::Diverged, None),
        (Some(RefError::Other(_)), _) => (RunOutcome::Failed, None),
        (None, _) if !build.gates_passed() => (RunOutcome::Blocked, None),
        (None, Some(Snapshot::Unchanged { commit, tree })) => (RunOutcome::Unchanged, Some((commit, tree))),
        (None, Some(Snapshot::New { commit, tree, .. })) => (RunOutcome::Published, Some((commit, tree))),
        // Gates passed and still no snapshot: only a missing publisher does
        // that, and its gate doesn't pass. A failure, not a panic, if a
        // later change ever makes it reachable.
        (None, None) => (RunOutcome::Failed, None),
    };
    RefRun {
        ref_name: build.ref_name.clone(),
        private_head: to_hex(&build.private_head),
        previous_public_head: build.remote_head.clone(),
        public_head: head.map(|(commit, _)| to_hex(&commit)),
        public_tree: head.map(|(_, tree)| to_hex(&tree)),
        published_files: build.published_files,
        excluded_entries: build.excluded_entries,
        gates: build.gates.iter().cloned().map(GateRecord::from).collect(),
        outcome,
        error: build.error.as_ref().map(|e| stored_error(&e.to_string())),
    }
}

/// A branch planned as published was pushed only if the run was.
fn settle_ref_outcomes(run: &mut PublishRun) {
    if run.outcome == RunOutcome::Published {
        return;
    }
    for r in &mut run.refs {
        if r.outcome == RunOutcome::Published {
            r.outcome = RunOutcome::Failed;
            r.public_head = None;
            r.public_tree = None;
        }
    }
}

fn failed_gates(gates: &[GateReport]) -> String {
    gates
        .iter()
        .filter(|g| !g.passed())
        .map(|g| format!("{:?}", g.gate))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The host gate's refusal, `None` if it passed.
fn host_summary(host: &GateReport) -> Option<String> {
    if host.passed() {
        return None;
    }
    let errors: Vec<&str> = host
        .findings
        .iter()
        .filter_map(|f| match f {
            Finding::HostNotAllowed { error } => Some(error.as_str()),
            _ => None,
        })
        .collect();
    Some(errors.join("; "))
}

async fn has_commit(repo: &Path, sha: &str) -> Result<bool> {
    let status = tokio::process::Command::new("git")
        .env("GIT_DIR", repo)
        .args(["cat-file", "-e", "--end-of-options"])
        .arg(format!("{sha}^{{commit}}"))
        .kill_on_drop(true)
        .status()
        .await
        .context("couldn't run git cat-file")?;
    Ok(status.success())
}

/// Stores the run and what it means for the mirror's state. Only a branch
/// the public side now holds at a snapshot of this mirror moves `heads`, so
/// a failed run can never make the next one build on something it didn't
/// push.
async fn record(ctx: &AppContext, resolved: &ResolvedRepo, mirror: &PublishMirror, run: &PublishRun) -> Result<()> {
    log::update(ctx, &resolved.uid, &mirror.id, |log| {
        for r in &run.refs {
            let status = match r.outcome {
                RunOutcome::Published | RunOutcome::Unchanged => RefSyncStatus::InSync,
                RunOutcome::Diverged => RefSyncStatus::Diverged,
                RunOutcome::Blocked | RunOutcome::Failed => {
                    RefSyncStatus::Error(r.error.clone().or_else(|| run.error.clone()).unwrap_or_default())
                }
            };
            log.ref_status.insert(r.ref_name.clone(), status);
            if let (RunOutcome::Published | RunOutcome::Unchanged, Some(commit), Some(tree)) =
                (r.outcome, &r.public_head, &r.public_tree)
            {
                log.heads.insert(
                    r.ref_name.clone(),
                    PublicHead {
                        commit: commit.clone(),
                        tree: tree.clone(),
                    },
                );
                log.pending.remove(&r.ref_name);
            }
        }
        log.last_error = run.error.clone();
        log.push_run(run.clone());
    })
    .await
}
