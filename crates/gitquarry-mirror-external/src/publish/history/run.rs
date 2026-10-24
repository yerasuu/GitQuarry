//! One history publish run, end to end, after the host gate: plan → load the
//! mapping → rewrite → write and gate → store → push → (the caller records).
//!
//! Within an epoch every push is a plain fast-forward: a public ref that
//! would not fast-forward is an error here, before the push, and the remote
//! refuses it anyway. Only a new epoch, which only a confirmed edit creates
//! ([`crate::publish::epoch`]), replaces public refs, and then with a lease
//! on exactly the value this mirror recorded there.
//!
//! The private repo's lock is held from the plan to the last object written
//! into the publish repo, and released before anything is uploaded to the
//! bucket or sent to the remote. The rewrite reads the private repo
//! throughout, so the lock can't be shorter without copying the repo; it is
//! the shared lock whenever the local mirror is fresh, so it holds up
//! pushes to the private repo, never reads.

use super::build::{self, Built, Outgoing};
use super::{Plan, Rewrite, fresh_entries};
use crate::git_remote;
use crate::publish::epoch::{Fingerprint, config_fingerprint, published_epoch};
use crate::publish::log::{self, GateRecord, PublicHead, PublishLog, PublishRun, RefRun, RunOutcome, stored_error};
use crate::publish::mapping::{Table, epoch_prefix, pack_store};
use crate::publish::objects::{Oid, count_reachable, from_hex, to_hex};
use crate::publish::preview::is_published_ref;
use crate::publish::scratch::{init_scratch, pack_objects};
use crate::publish::{PublishMirror, audit::GateReport};
use anyhow::{Context, Result, bail};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// Runs a history publish of `mirror`. `remote_refs` is the public side as
/// `ls-remote` saw it; `log` is the mirror's log as the run started.
/// Returns the fingerprint of what the public side holds afterwards.
///
/// # Errors
///
/// Whatever stopped the run; `run` says how far it got.
pub(in crate::publish) async fn execute(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &PublishMirror,
    run: &mut PublishRun,
    remote_refs: &HashMap<String, String>,
    log: &PublishLog,
) -> Result<Fingerprint> {
    let epoch = mirror.filter_epoch;
    let published = published_epoch(log);
    if let Some(published) = published
        && epoch < published
    {
        bail!("the mirror is at epoch {epoch}, behind epoch {published} already published; nothing was pushed");
    }
    let new_epoch = published != Some(epoch);
    if !new_epoch && let Some(fingerprint) = unchanged(resolved, mirror, remote_refs, log, run).await? {
        return Ok(fingerprint);
    }

    // Removed with everything in it when the run ends, however it ends.
    let scratch = gitquarry_core::git::scratch_dir(&std::env::temp_dir())?;
    let workspace = Workspace {
        repo: scratch.path().join("public.git"),
        stage: scratch.path().join("stage"),
    };
    init_scratch(&workspace.repo)?;
    std::fs::create_dir_all(&workspace.stage).with_context(|| format!("creating {}", workspace.stage.display()))?;
    // The epoch's publish repo, from the bucket, before the private repo's
    // lock is taken: this is the slow part when the public history is large.
    let store = pack_store(ctx, &resolved.uid, &mirror.id, epoch);
    let pack_dir = workspace.repo.join("objects").join("pack");
    for name in store.list_pack_names().await? {
        store.download_pack(&name, &pack_dir).await?;
    }
    let prefix = epoch_prefix(&resolved.uid, &mirror.id, epoch);
    let mut map = Table::<40>::new(format!("{prefix}/map"));
    let mut excluded = Table::<0>::new(format!("{prefix}/excluded"));
    let known: Vec<Oid> = if new_epoch {
        Vec::new()
    } else {
        log.heads
            .values()
            .filter_map(|head| head.private.as_deref())
            .filter_map(|private| from_hex(private).ok())
            .collect()
    };

    let local = ctx.open_mirror_read(resolved).await?;
    let git_dir = Arc::new(local.path.clone());
    let mut plan = plan(&git_dir, mirror, known.clone()).await?;
    map.load(ctx, plan.shards()).await?;
    if !known.is_empty() && !plan.closed_over(&map) {
        warn!(
            target: "publish",
            repo = %resolved.uid,
            mirror = %mirror.id,
            epoch,
            "mapping incomplete for the new commits, rewriting from the root"
        );
        plan = self::plan(&git_dir, mirror, Vec::new()).await?;
        map.load_all(ctx).await?;
    }
    if !new_epoch && log.fingerprint.as_ref() != Some(&plan.fingerprint) {
        run.outcome = RunOutcome::Blocked;
        bail!(
            "the filter epoch {epoch} was published with has changed since (the mirror's settings or a \
             .gitquarry/publish.toml); nothing was pushed: confirm an epoch bump to rewrite the public history"
        );
    }

    let plan = Arc::new(plan);
    let map = Arc::new(map);
    let (rewrite, total) = {
        let (git_dir, plan, map) = (Arc::clone(&git_dir), Arc::clone(&plan), Arc::clone(&map));
        let identities = mirror.identities.clone();
        tokio::task::spawn_blocking(move || {
            let rewrite = super::rewrite(&git_dir, &plan, &identities, &map)?;
            let heads: Vec<Oid> = plan.refs.iter().filter_map(|r| r.commit).collect();
            Ok::<_, anyhow::Error>((rewrite, count_reachable(&git_dir, &heads)?))
        })
        .await
        .context("the publish task panicked")??
    };
    run.commits_rewritten = rewrite.rewriter.fresh.len();
    run.commits_collapsed = rewrite.rewriter.collapsed;
    run.commits_reused = total.saturating_sub(run.commits_rewritten);

    let decided = decide(&rewrite, remote_refs, log, new_epoch);
    run.refs = rewrite
        .refs
        .iter()
        .zip(&decided)
        .map(|(r, d)| ref_run(r, d, remote_refs))
        .collect();
    if run.refs.is_empty() {
        bail!("no branch or tag of this repo matches the mirror's published refs");
    }
    if let Some((r, d, error)) = rewrite
        .refs
        .iter()
        .zip(&decided)
        .find_map(|(r, d)| d.error().map(|e| (r, d, e)))
    {
        if matches!(d, Decision::Foreign(_)) {
            run.outcome = RunOutcome::Diverged;
        }
        bail!("{}: {error}", r.name);
    }
    let pushing: Vec<usize> = decided
        .iter()
        .enumerate()
        .filter(|(_, d)| d.pushes())
        .map(|(i, _)| i)
        .collect();

    let rewrite = Arc::new(rewrite);
    let outgoing = {
        let (repo, rewrite, pushing) = (workspace.repo.clone(), Arc::clone(&rewrite), pushing.clone());
        tokio::task::spawn_blocking(move || build::outgoing(&repo, &rewrite, &pushing))
            .await
            .context("the publish task panicked")??
    };
    excluded.load(ctx, build::excluded_shards(&outgoing)).await?;
    let excluded = Arc::new(excluded);
    let built = {
        let step = WriteStep {
            git_dir: Arc::clone(&git_dir),
            repo: workspace.repo.clone(),
            stage: workspace.stage.clone(),
            rewrite: Arc::clone(&rewrite),
            outgoing: Arc::new(outgoing),
            pushing: pushing.clone(),
            excluded: Arc::clone(&excluded),
            mirror: mirror.clone(),
            plan: Arc::clone(&plan),
        };
        Arc::new(
            tokio::task::spawn_blocking(move || step.run())
                .await
                .context("the publish task panicked")??,
        )
    };
    drop(local);

    for (i, gates) in pushing.iter().zip(&built.gates) {
        run.refs[*i].gates = gates.iter().cloned().map(GateRecord::from).collect();
        if !gates.iter().all(GateReport::passed) {
            run.refs[*i].outcome = RunOutcome::Blocked;
        }
    }
    let blocked: Vec<String> = pushing
        .iter()
        .zip(&built.gates)
        .filter(|(_, gates)| !gates.iter().all(GateReport::passed))
        .map(|(i, gates)| {
            format!(
                "{} ({})",
                rewrite.refs[*i].name,
                crate::publish::run::failed_gates(gates)
            )
        })
        .collect();
    if !blocked.is_empty() {
        run.outcome = RunOutcome::Blocked;
        bail!("blocked by the gates, nothing was pushed: {}", blocked.join("; "));
    }
    for (i, d) in decided.iter().enumerate() {
        if let Decision::FastForward(from) = d {
            let to = rewrite.refs[i].public.map(|oid| to_hex(&oid)).unwrap_or_default();
            if !is_ancestor(&workspace.repo, from, &to).await? {
                bail!(
                    "{}: {to} doesn't fast-forward {from}, the public head of epoch {epoch}: the private history \
                     was rewritten; nothing was pushed, and nothing is forced within an epoch",
                    rewrite.refs[i].name
                );
            }
        }
    }
    {
        let (repo, built, rewrite, excluded) = (
            workspace.repo.clone(),
            Arc::clone(&built),
            Arc::clone(&rewrite),
            Arc::clone(&excluded),
        );
        tokio::task::spawn_blocking(move || build::verify(&repo, &built, &rewrite, &excluded))
            .await
            .context("the publish task panicked")??;
    }

    // Stored before the push, and in this order, so that the bucket never
    // describes an object it doesn't hold: the pack, the blobs the epoch has
    // seen excluded, then the mapping that says which commits are done.
    let packed = {
        let (repo, stage, written) = (workspace.repo.clone(), workspace.stage.clone(), built.written.clone());
        tokio::task::spawn_blocking(move || pack_objects(&repo, &written, &stage))
            .await
            .context("the publish task panicked")??
    };
    if let Some((base, pack, idx)) = &packed {
        store.upload_pack(base, pack, idx).await?;
    }
    Table::<0>::new(format!("{prefix}/excluded"))
        .store(ctx, &build::fresh_excluded(&rewrite))
        .await?;
    Table::<40>::new(format!("{prefix}/map"))
        .store(ctx, &fresh_entries(&rewrite.rewriter))
        .await?;

    if pushing.is_empty() {
        run.outcome = RunOutcome::Unchanged;
        return Ok(plan.fingerprint.clone());
    }
    let mut refspecs = Vec::with_capacity(pushing.len());
    let mut leases = Vec::new();
    for i in &pushing {
        let r = &rewrite.refs[*i];
        let public = r.public.map(|oid| to_hex(&oid)).unwrap_or_default();
        refspecs.push(format!("{public}:{}", r.name));
        if let Decision::Force(expected) = &decided[*i] {
            leases.push((r.name.clone(), expected.clone()));
        }
    }
    log::update(ctx, &resolved.uid, &mirror.id, |log| {
        for i in &pushing {
            let r = &rewrite.refs[*i];
            if let (Some(public), Some(tree)) = (r.public, r.public_tree) {
                log.pending.insert(
                    r.name.clone(),
                    PublicHead {
                        commit: to_hex(&public),
                        tree: to_hex(&tree),
                        private: Some(to_hex(&r.private_object)),
                    },
                );
            }
        }
    })
    .await
    .context("recording the refs about to be pushed")?;
    run.forced = !leases.is_empty();
    info!(
        target: "publish",
        repo = %resolved.uid,
        mirror = %mirror.id,
        epoch,
        refs = pushing.len(),
        forced = run.forced,
        rewritten = run.commits_rewritten,
        "pushing history"
    );
    git_remote::push_atomic_with_leases(
        &workspace.repo,
        &mirror.remote_url,
        mirror.token.expose(),
        &leases,
        &refspecs,
    )
    .await
    .context("the public remote refused the push; nothing was published")?;
    run.outcome = RunOutcome::Published;
    Ok(plan.fingerprint.clone())
}

/// The publish repo and its staging area, inside the run's scratch dir.
struct Workspace {
    repo: PathBuf,
    stage: PathBuf,
}

async fn plan(git_dir: &Arc<PathBuf>, mirror: &PublishMirror, known: Vec<Oid>) -> Result<Plan> {
    let (git_dir, mirror) = (Arc::clone(git_dir), mirror.clone());
    tokio::task::spawn_blocking(move || super::plan(&git_dir, &mirror, &known))
        .await
        .context("the publish task panicked")?
}

/// Everything [`build::write_and_gate`] needs, owned, for its blocking
/// thread.
struct WriteStep {
    git_dir: Arc<PathBuf>,
    repo: PathBuf,
    stage: PathBuf,
    rewrite: Arc<Rewrite>,
    outgoing: Arc<Outgoing>,
    pushing: Vec<usize>,
    excluded: Arc<Table<0>>,
    mirror: PublishMirror,
    plan: Arc<Plan>,
}

impl WriteStep {
    fn run(self) -> Result<Built> {
        build::write_and_gate(
            &build::Paths {
                private_git_dir: &self.git_dir,
                repo: &self.repo,
                stage: &self.stage,
            },
            &self.rewrite,
            &self.outgoing,
            &self.pushing,
            &self.excluded,
            &self.mirror,
            self.plan.repo_file_error.as_deref(),
        )
    }
}

/// What a run does with one ref.
enum Decision {
    /// The public ref already holds the rewritten value.
    Unchanged,
    /// A new ref, or one this mirror never pushed and nobody else created.
    Create,
    /// Within the epoch, from the public value this mirror recorded.
    FastForward(String),
    /// A new epoch over a value this mirror recorded: replaced under a
    /// lease on exactly that value.
    Force(String),
    /// The public ref is somewhere this mirror never pushed it.
    Foreign(String),
    /// A tag already published within this epoch, now at another value.
    TagMoved(String),
    Error(String),
}

impl Decision {
    fn pushes(&self) -> bool {
        matches!(self, Decision::Create | Decision::FastForward(_) | Decision::Force(_))
    }

    fn error(&self) -> Option<String> {
        match self {
            Decision::Foreign(head) => Some(format!(
                "the public ref is at {head}, which this mirror didn't publish; not building on it and not \
                 forcing over it"
            )),
            Decision::TagMoved(head) => Some(format!(
                "the public tag is at {head} and would move within the epoch; tags are never forced within an \
                 epoch"
            )),
            Decision::Error(error) => Some(error.clone()),
            Decision::Unchanged | Decision::Create | Decision::FastForward(_) | Decision::Force(_) => None,
        }
    }
}

fn decide(
    rewrite: &Rewrite,
    remote_refs: &HashMap<String, String>,
    log: &PublishLog,
    new_epoch: bool,
) -> Vec<Decision> {
    rewrite
        .refs
        .iter()
        .map(|r| {
            if let Some(error) = &r.error {
                return Decision::Error(error.clone());
            }
            let Some(public) = r.public.map(|oid| to_hex(&oid)) else {
                return Decision::Error("nothing to publish".to_string());
            };
            let Some(remote) = remote_refs.get(&r.name) else {
                return Decision::Create;
            };
            if *remote == public {
                return Decision::Unchanged;
            }
            let ours = log
                .heads
                .get(&r.name)
                .into_iter()
                .chain(log.pending.get(&r.name))
                .any(|known| known.commit == *remote);
            match (ours, new_epoch, r.name.starts_with("refs/tags/")) {
                (false, _, _) => Decision::Foreign(remote.clone()),
                (true, true, _) => Decision::Force(remote.clone()),
                (true, false, true) => Decision::TagMoved(remote.clone()),
                (true, false, false) => Decision::FastForward(remote.clone()),
            }
        })
        .collect()
}

fn ref_run(r: &super::RefResult, decision: &Decision, remote_refs: &HashMap<String, String>) -> RefRun {
    let outcome = match decision {
        Decision::Unchanged => RunOutcome::Unchanged,
        Decision::Create | Decision::FastForward(_) | Decision::Force(_) => RunOutcome::Published,
        Decision::Foreign(_) => RunOutcome::Diverged,
        Decision::TagMoved(_) | Decision::Error(_) => RunOutcome::Failed,
    };
    let planned = matches!(outcome, RunOutcome::Unchanged | RunOutcome::Published);
    RefRun {
        ref_name: r.name.clone(),
        private_head: to_hex(&r.private_object),
        previous_public_head: remote_refs.get(&r.name).cloned(),
        public_head: r.public.filter(|_| planned).map(|oid| to_hex(&oid)),
        public_tree: r.public_tree.filter(|_| planned).map(|oid| to_hex(&oid)),
        published_files: r.published_files,
        excluded_entries: r.excluded_entries,
        gates: Vec::new(),
        outcome,
        error: decision.error().map(|e| stored_error(&e)),
    }
}

/// The run that has nothing to do: every published ref's private value is
/// the one this epoch last published, the public side still holds what
/// this mirror pushed, and the mirror's own settings didn't change. Decided
/// from the bucket's manifest and the log alone, without the private repo,
/// the publish repo or the mapping. `None` when anything differs.
async fn unchanged(
    resolved: &ResolvedRepo,
    mirror: &PublishMirror,
    remote_refs: &HashMap<String, String>,
    log: &PublishLog,
    run: &mut PublishRun,
) -> Result<Option<Fingerprint>> {
    let Some(fingerprint) = log
        .fingerprint
        .as_ref()
        .filter(|f| f.config == config_fingerprint(mirror))
    else {
        return Ok(None);
    };
    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    let mut refs = Vec::new();
    for (name, value) in manifest.refs.iter().filter(|(name, _)| is_published_ref(mirror, name)) {
        let Some(head) = log.heads.get(name) else {
            return Ok(None);
        };
        if head.private.as_deref() != Some(value.as_str()) || remote_refs.get(name) != Some(&head.commit) {
            return Ok(None);
        }
        let last = log
            .runs
            .iter()
            .flat_map(|run| &run.refs)
            .find(|r| r.ref_name == *name && r.public_head.as_deref() == Some(head.commit.as_str()));
        refs.push(RefRun {
            ref_name: name.clone(),
            private_head: value.clone(),
            previous_public_head: Some(head.commit.clone()),
            public_head: Some(head.commit.clone()),
            public_tree: Some(head.tree.clone()),
            published_files: last.map_or(0, |r| r.published_files),
            excluded_entries: last.map_or(0, |r| r.excluded_entries),
            gates: Vec::new(),
            outcome: RunOutcome::Unchanged,
            error: None,
        });
    }
    if refs.is_empty() {
        return Ok(None);
    }
    run.refs = refs;
    run.outcome = RunOutcome::Unchanged;
    Ok(Some(fingerprint.clone()))
}

/// `git merge-base --is-ancestor` in the publish repo, which holds every
/// public commit of the epoch.
async fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let status = tokio::process::Command::new("git")
        .env("GIT_DIR", repo)
        .args(["merge-base", "--is-ancestor", "--end-of-options", ancestor, descendant])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .context("couldn't run git merge-base")?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base couldn't compare {ancestor} with {descendant} in the publish repo"),
    }
}
