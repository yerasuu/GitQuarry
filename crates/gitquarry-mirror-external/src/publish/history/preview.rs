//! The dry run of a history publish (`docs/publish-mirror-concept.md` §6):
//! the same plan and in-memory rewrite a run makes, seeded from the same
//! mapping, so the public heads shown are the ones a publish would push,
//! with every gate run over the commits a publish would rewrite. Writes no
//! object, stores nothing and asks nothing of the public remote, so whether
//! the public side has moved is only known from the log.
//!
//! The gates differ from a run's in what they can't know without the
//! publish repo: the secret scan reads every blob of the new commits rather
//! than only those the publish repo lacks, and the blob-set audit compares
//! against what this rewrite saw excluded, not the epoch's stored set.

use super::{Plan, RefResult, Rewrite};
use crate::publish::PublishMirror;
use crate::publish::audit;
use crate::publish::epoch::published_epoch;
use crate::publish::log::PublishLog;
use crate::publish::mapping::{Table, epoch_prefix};
use crate::publish::objects::{ObjectKind, ObjectReader, Oid, count_reachable, from_hex, parse_commit, to_hex};
use crate::publish::preview::{MAX_PREVIEW_COMMITS, MAX_PREVIEW_PATHS, RefPreview, count_exclusions, published_paths};
use anyhow::{Context, Result};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

pub(in crate::publish) struct HistoryPreview {
    pub refs: Vec<RefPreview>,
    pub force_push: bool,
    pub epoch_bump_needed: bool,
}

/// # Errors
///
/// Fails when the local mirror or the bucket can't be read; a problem with
/// one ref is reported on that ref.
pub(in crate::publish) async fn preview(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &PublishMirror,
    log: &PublishLog,
) -> Result<HistoryPreview> {
    let epoch = mirror.filter_epoch;
    let published = published_epoch(log);
    let new_epoch = published != Some(epoch);
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
    let mut map = Table::<40>::new(format!("{}/map", epoch_prefix(&resolved.uid, &mirror.id, epoch)));
    if !mirror.id.is_empty() {
        map.load(ctx, plan.shards()).await?;
        if !known.is_empty() && !plan.closed_over(&map) {
            plan = self::plan(&git_dir, mirror, Vec::new()).await?;
            map.load_all(ctx).await?;
        }
    }
    let epoch_bump_needed = !new_epoch && log.fingerprint.as_ref() != Some(&plan.fingerprint);
    let force_push = new_epoch && plan.refs.iter().any(|r| log.heads.contains_key(&r.name));
    let (mirror, log) = (mirror.clone(), log.clone());
    let refs = tokio::task::spawn_blocking(move || {
        if plan.candidates() > MAX_PREVIEW_COMMITS {
            return windowed(&git_dir, &plan, &mirror, &log);
        }
        let rewrite = super::rewrite(&git_dir, &plan, &mirror.identities, &map)?;
        let same_epoch = !new_epoch;
        rewrite
            .refs
            .iter()
            .map(|r| {
                ref_preview(
                    &git_dir,
                    &rewrite,
                    r,
                    &mirror,
                    &log,
                    same_epoch,
                    plan.repo_file_error.as_deref(),
                )
            })
            .collect::<Result<Vec<_>>>()
    })
    .await
    .context("the preview task panicked")??;
    drop(local);
    Ok(HistoryPreview {
        refs,
        force_push,
        epoch_bump_needed,
    })
}

async fn plan(git_dir: &Arc<std::path::PathBuf>, mirror: &PublishMirror, known: Vec<Oid>) -> Result<Plan> {
    let (git_dir, mirror) = (Arc::clone(git_dir), mirror.clone());
    tokio::task::spawn_blocking(move || super::plan(&git_dir, &mirror, &known))
        .await
        .context("the preview task panicked")?
}

/// More new commits than a preview rewrites: the snapshot preview's window
/// over each ref, flagged truncated, with no public head since the window's
/// ids aren't the real ones.
fn windowed(git_dir: &Path, plan: &Plan, mirror: &PublishMirror, log: &PublishLog) -> Result<Vec<RefPreview>> {
    let mut refs = crate::publish::preview::preview_refs(git_dir, mirror, log)?;
    for r in &mut refs {
        r.public_head = None;
        r.commits_to_rewrite = plan
            .refs
            .iter()
            .find(|p| p.name == r.ref_name)
            .map_or(0, |p| p.commits.len());
    }
    Ok(refs)
}

fn ref_preview(
    git_dir: &Path,
    rewrite: &Rewrite,
    r: &RefResult,
    mirror: &PublishMirror,
    log: &PublishLog,
    same_epoch: bool,
    repo_file_error: Option<&str>,
) -> Result<RefPreview> {
    let mut preview = RefPreview {
        ref_name: r.name.clone(),
        private_head: to_hex(&r.private_object),
        public_head: r.public.map(|oid| to_hex(&oid)),
        commits_to_rewrite: r.rewritten,
        collapsed_commits: r.collapsed,
        published_commits: r.published.len(),
        error: r.error.clone(),
        ..Default::default()
    };
    let (Some(public), Some(tree), None) = (r.public, r.public_tree, &r.error) else {
        return Ok(preview);
    };
    let rewriter = &rewrite.rewriter;
    preview.unchanged = same_epoch && log.heads.get(&r.name).is_some_and(|h| h.commit == to_hex(&public));
    let mut reader = ObjectReader::open(git_dir)?;
    let private_commit = r
        .private_commit
        .context("a ref with a public head has a private commit")?;
    preview.commits_reused = count_reachable(git_dir, &[private_commit])?.saturating_sub(r.rewritten);
    let private_tree = parse_commit(&reader.read_kind(&private_commit, ObjectKind::Commit)?)?.tree;
    let mut excluded = BTreeMap::new();
    count_exclusions(
        &mut reader,
        rewriter.filter(),
        String::new(),
        private_tree,
        false,
        &mut excluded,
    )?;
    preview.excluded = excluded;
    let mut paths = published_paths(rewriter, tree);
    preview.published_paths_total = paths.len();
    paths.truncate(MAX_PREVIEW_PATHS);
    preview.published_paths = paths;

    let commits = &rewriter.published[r.published.clone()];
    let roots = commits.iter().map(|c| c.tree).chain([tree]);
    let set = audit::published_set_from(roots, &rewriter.public_trees, rewriter.filter());
    let messages = commits
        .iter()
        .map(|c| (c.private, c.message()))
        .chain(r.tag.as_ref().map(|tag| (r.private_object, tag.message())));
    let identities = commits
        .iter()
        .map(|c| (c.private, c.unmapped.as_slice()))
        .chain(r.tag.as_ref().map(|tag| (r.private_object, tag.unmapped.as_slice())));
    preview.gates = vec![
        audit::filter_gate(&set, repo_file_error.map(str::to_string)),
        audit::blob_set_gate(&set, rewriter),
        audit::license_gate(&mut reader, rewriter, Some(tree), mirror.spdx.as_deref())?,
        audit::secret_gate(&mut reader, &set, messages, &mirror.secret_allowlist)?,
        audit::identity_gate_over(identities, mirror.unmapped),
    ];
    Ok(preview)
}
