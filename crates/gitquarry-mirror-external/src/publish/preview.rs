//! The dry run of `docs/publish-mirror-concept.md` §6: for the head of every
//! publishable ref, what would be published and what every gate says about
//! it. Reads the repo's local mirror and nothing else; computes the public
//! ids in memory (see [`super::rewrite`]), so there is no scratch repository
//! to clean up and nothing that a crash halfway could leave behind.

use super::audit::{self, GateReport};
use super::filter::{Decision, ExcludedBy, Filter, REPO_FILTER_PATH, RepoFilterFile};
use super::log::{self, PublishLog};
use super::objects::{Commit, ObjectKind, ObjectReader, Oid, list_branches_and_tags, parse_tree, to_hex, topo_order};
use super::rewrite::{EMPTY_TREE, Rewriter, entry_kind, join};
use super::snapshot::{Snapshot, plan_snapshot};
use super::{NEVER_PUBLISHED_REF_PREFIXES, PublishMirror, PublishMode};
use anyhow::{Context, Result};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::git::codeowners::segments_match;
use std::collections::BTreeMap;
use std::path::Path;

/// The whole preview of one mirror.
#[derive(Debug)]
pub struct Preview {
    pub host: GateReport,
    pub refs: Vec<RefPreview>,
}

impl Preview {
    /// Whether a publish run would be allowed to go ahead: every gate of
    /// every ref clean, and at least one ref to publish.
    pub fn passed(&self) -> bool {
        self.host.passed()
            && !self.refs.is_empty()
            && self
                .refs
                .iter()
                .all(|r| r.error.is_none() && r.gates.iter().all(GateReport::passed))
    }
}

/// One publishable ref at its current head.
#[derive(Debug, Default)]
pub struct RefPreview {
    pub ref_name: String,
    pub private_head: String,
    /// What a publish would leave on the public branch, by the mirror's
    /// mode: the snapshot commit, or the one already there when
    /// [`Self::unchanged`]. `None` when the filter leaves nothing, or when
    /// the mode has no publisher to author the commit.
    pub public_head: Option<String>,
    /// A publish would push nothing: the filtered tree is the one the last
    /// recorded snapshot holds.
    pub unchanged: bool,
    /// Every file and gitlink path in the public head tree, sorted.
    pub published_paths: Vec<String>,
    /// Entries excluded from the head tree, by the rule that excluded them.
    /// An excluded directory counts once: its children are never read.
    pub excluded: BTreeMap<ExcludedBy, usize>,
    pub published_commits: usize,
    /// Commits that only touched excluded paths and so never appear.
    pub collapsed_commits: usize,
    pub gates: Vec<GateReport>,
    /// Why this ref couldn't be previewed at all.
    pub error: Option<String>,
}

/// Runs every gate in dry-run against the repo's current refs.
///
/// # Errors
///
/// Fails when the local mirror can't be opened or read; a problem with one
/// ref is reported on that ref's [`RefPreview::error`] instead.
pub async fn preview(ctx: &AppContext, resolved: &ResolvedRepo, mirror: &PublishMirror) -> Result<Preview> {
    let host = audit::host_gate(&mirror.remote_url, &super::allowed_hosts(ctx).await?);
    let local = ctx.open_mirror_read(resolved).await?;
    let mirror = mirror.clone();
    // Thousands of synchronous pipe round trips, so off the async workers;
    // `local` moves in with it so the repo lock is held until it's done.
    // A mirror that was never saved has no id to key a log by, and nothing
    // published either.
    let log = if mirror.id.is_empty() {
        PublishLog::default()
    } else {
        log::get(ctx, &resolved.uid, &mirror.id).await?
    };
    let refs = tokio::task::spawn_blocking(move || preview_refs(&local.path, &mirror, &log))
        .await
        .context("the preview task panicked")??;
    Ok(Preview { host, refs })
}

/// The refs a mirror publishes: branches and tags matching its declared
/// patterns, nothing else.
pub(super) fn selected_refs(git_dir: &Path, mirror: &PublishMirror) -> Result<Vec<(String, Option<Oid>)>> {
    let refs = list_branches_and_tags(git_dir)?;
    Ok(refs
        .into_iter()
        .filter(|(name, _)| !NEVER_PUBLISHED_REF_PREFIXES.iter().any(|p| name.starts_with(p)))
        .filter(|(name, _)| {
            let (short, patterns) = match (name.strip_prefix("refs/heads/"), name.strip_prefix("refs/tags/")) {
                (Some(branch), _) => (branch, &mirror.publish_branches),
                (None, Some(tag)) => (tag, &mirror.publish_tags),
                (None, None) => return false,
            };
            patterns.iter().any(|p| segments_match(p, short))
        })
        .collect())
}

fn preview_refs(git_dir: &Path, mirror: &PublishMirror, log: &PublishLog) -> Result<Vec<RefPreview>> {
    let mut reader = ObjectReader::open(git_dir)?;
    let mut previews = Vec::new();
    for (ref_name, head) in selected_refs(git_dir, mirror)? {
        let preview = match head {
            Some(head) => {
                preview_ref(&mut reader, git_dir, mirror, log, &ref_name, head).unwrap_or_else(|e| RefPreview {
                    ref_name: ref_name.clone(),
                    private_head: to_hex(&head),
                    error: Some(format!("{e:#}")),
                    ..Default::default()
                })
            }
            None => RefPreview {
                ref_name: ref_name.clone(),
                error: Some("doesn't point at a commit".to_string()),
                ..Default::default()
            },
        };
        previews.push(preview);
    }
    Ok(previews)
}

fn preview_ref(
    reader: &mut ObjectReader,
    git_dir: &Path,
    mirror: &PublishMirror,
    log: &PublishLog,
    ref_name: &str,
    head: Oid,
) -> Result<RefPreview> {
    let head_commit = super::objects::parse_commit(&reader.read_kind(&head, ObjectKind::Commit)?)?;
    let (repo_file, repo_file_error) = load_repo_filter(reader, head_commit.tree)?;
    let filter = Filter::new(&mirror.filter, repo_file.as_ref());
    let mut rewriter = Rewriter::new(&filter, &mirror.identities);
    for commit in topo_order(git_dir, &head)? {
        rewriter.rewrite_commit(reader, commit)?;
    }
    let full_head = rewriter.mapped(&head).flatten();
    let head_tree = full_head.and_then(|c| rewriter.public_tree_of(&c));
    let (public_head, unchanged) = match mirror.mode {
        PublishMode::Snapshot => snapshot_head(mirror, log, ref_name, &head_commit, head_tree)?,
    };

    let set = audit::published_set(&rewriter, &filter);
    let gates = vec![
        audit::filter_gate(&set, repo_file_error),
        audit::blob_set_gate(&set, &rewriter),
        audit::license_gate(reader, &rewriter, head_tree, mirror.spdx.as_deref())?,
        audit::secret_gate(
            reader,
            &set,
            rewriter.published.iter().map(|c| (c.private, c.message.as_slice())),
            &mirror.secret_allowlist,
        )?,
        audit::identity_gate(&rewriter, mirror.unmapped),
    ];
    let mut excluded = BTreeMap::new();
    count_exclusions(reader, &filter, String::new(), head_commit.tree, false, &mut excluded)?;
    Ok(RefPreview {
        ref_name: ref_name.to_string(),
        private_head: to_hex(&head),
        public_head: public_head.map(|c| to_hex(&c)),
        unchanged,
        published_paths: head_tree.map(|t| published_paths(&rewriter, t)).unwrap_or_default(),
        excluded,
        published_commits: rewriter.published.len(),
        collapsed_commits: rewriter.collapsed,
        gates,
        error: None,
    })
}

/// The commit a snapshot publish would leave on the branch and whether it is
/// already there, from the same [`plan_snapshot`] the run uses. The base is
/// the last snapshot the log recorded: the preview doesn't ask the remote.
fn snapshot_head(
    mirror: &PublishMirror,
    log: &PublishLog,
    ref_name: &str,
    head_commit: &Commit,
    head_tree: Option<Oid>,
) -> Result<(Option<Oid>, bool)> {
    let Some(tree) = head_tree.filter(|tree| *tree != EMPTY_TREE) else {
        return Ok((None, false));
    };
    let planned = plan_snapshot(
        mirror.publisher.as_ref(),
        ref_name,
        head_commit,
        tree,
        log.heads.get(ref_name),
    )?;
    Ok(match planned.snapshot {
        Some(Snapshot::Unchanged { commit, .. }) => (Some(commit), true),
        Some(Snapshot::New { commit, .. }) => (Some(commit), false),
        None => (None, false),
    })
}

/// The in-repo half of the filter from `root`, or why it couldn't be used:
/// an unusable file is a filter-gate finding, never silently ignored.
pub(super) fn load_repo_filter(
    reader: &mut ObjectReader,
    root: Oid,
) -> Result<(Option<RepoFilterFile>, Option<String>)> {
    Ok(match read_repo_filter(reader, root)? {
        None => (None, None),
        Some(bytes) => match RepoFilterFile::parse(&bytes) {
            Ok(file) => (Some(file), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        },
    })
}

/// `.gitquarry/publish.toml` in the source ref's head tree, if there is one.
fn read_repo_filter(reader: &mut ObjectReader, root: Oid) -> Result<Option<Vec<u8>>> {
    let mut tree = root;
    let mut parts = REPO_FILTER_PATH.split('/').peekable();
    while let Some(part) = parts.next() {
        let entries = parse_tree(&reader.read_kind(&tree, ObjectKind::Tree)?)?;
        let Some(entry) = entries.into_iter().find(|e| e.name == part.as_bytes()) else {
            return Ok(None);
        };
        match (parts.peek().is_some(), entry.is_tree()) {
            (true, true) => tree = entry.oid,
            (false, false) if !entry.is_gitlink() => {
                return Ok(Some(reader.read_kind(&entry.oid, ObjectKind::Blob)?));
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// The head tree walked with the filter only, to count what it excludes and
/// why. Separate from the rewrite because the rewrite's cache skips every
/// subtree it has already seen in an earlier commit.
pub(super) fn count_exclusions(
    reader: &mut ObjectReader,
    filter: &Filter,
    dir: String,
    tree: Oid,
    included: bool,
    counts: &mut BTreeMap<ExcludedBy, usize>,
) -> Result<()> {
    for entry in parse_tree(&reader.read_kind(&tree, ObjectKind::Tree)?)? {
        let path = join(&dir, &entry.name_lossy());
        match filter.decide(&path, entry_kind(&entry), included) {
            Decision::Exclude(by) => *counts.entry(by).or_default() += 1,
            Decision::Keep { included } if entry.is_tree() => {
                count_exclusions(reader, filter, path, entry.oid, included, counts)?;
            }
            Decision::Keep { .. } => {}
        }
    }
    Ok(())
}

pub(super) fn published_paths(rewriter: &Rewriter<'_>, root: Oid) -> Vec<String> {
    let mut paths = Vec::new();
    let mut stack = vec![(String::new(), root)];
    while let Some((dir, tree)) = stack.pop() {
        for entry in rewriter.public_trees.get(&tree).into_iter().flatten() {
            let path = join(&dir, &entry.name_lossy());
            if entry.is_tree() {
                stack.push((path, entry.oid));
            } else {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}
