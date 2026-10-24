//! History publish (`docs/publish-mirror-concept.md` §3–§5, phase 2): every
//! commit reachable from the published refs rewritten through the filter,
//! incrementally, against a mapping that survives between runs.
//!
//! A run is a chain of synchronous steps over the private repo and the
//! publish repo, with the bucket reads they need in between ([`run`]):
//! [`plan`] lists what is new since the heads this mirror last published,
//! the mapping shards those commits need are loaded, [`rewrite`] computes
//! every public id in memory, [`build`] writes the objects that are
//! actually new into the publish repo and runs every gate on them. Only
//! then is anything stored or pushed.
//!
//! The publish repo is per mirror and per epoch, never the private repo's
//! object store: it starts as a fresh `git init`, receives the packs earlier
//! runs of the same epoch stored in the bucket ([`super::mapping`]) and the
//! objects this run writes, and nothing else. No clone, no fetch of the
//! private repo, no alternates.

mod build;
pub(super) mod preview;
pub(super) mod run;
mod tags;

use super::epoch::{Fingerprint, config_fingerprint, repo_files_fingerprint};
use super::filter::{Filter, RepoFilterFile};
use super::mapping::{MapValue, Table, from_map_value};
use super::objects::{ObjectKind, ObjectReader, Oid, commits_since, parse_commit, to_hex};
use super::preview::{count_exclusions, load_repo_filter, published_paths, selected_refs};
use super::rewrite::{EMPTY_TREE, Rewriter};
use super::{IdentityMapping, PublishMirror};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;
use std::path::Path;
use tags::PublicTag;

/// One published ref, as found in the private repo.
pub(super) struct RefPlan {
    pub name: String,
    /// The ref's own value: a commit, or an annotated tag object.
    pub object: Oid,
    pub commit: Option<Oid>,
    /// Commits reachable from this ref, new since the known heads and not
    /// listed under an earlier ref; parents first, each with its parents.
    pub commits: Vec<(Oid, Vec<Oid>)>,
}

/// What a run has to rewrite.
pub(super) struct Plan {
    pub refs: Vec<RefPlan>,
    /// The entity's filter joined with every published ref's in-repo file.
    /// One filter for the whole run, not one per ref as a snapshot uses: a
    /// private commit reachable from two refs must map to one public commit.
    pub filter: Filter,
    pub repo_file_error: Option<String>,
    pub fingerprint: Fingerprint,
}

impl Plan {
    pub(super) fn candidates(&self) -> usize {
        self.refs.iter().map(|r| r.commits.len()).sum()
    }

    /// The mapping shards the rewrite reads: every candidate, every parent
    /// and every ref head.
    pub(super) fn shards(&self) -> BTreeSet<u8> {
        let mut shards = BTreeSet::new();
        for r in &self.refs {
            shards.extend(r.commit.map(|c| c[0]));
            for (commit, parents) in &r.commits {
                shards.insert(commit[0]);
                shards.extend(parents.iter().map(|p| p[0]));
            }
        }
        shards
    }

    /// Whether every commit the rewrite will meet is either new here or
    /// already in `map`. Not so when a mapping entry is missing (lost to a
    /// concurrent writer, or never stored by a run that failed between its
    /// push and its record): then the run has to start over from the root.
    pub(super) fn closed_over(&self, map: &Table<40>) -> bool {
        let candidates: HashSet<&Oid> = self
            .refs
            .iter()
            .flat_map(|r| r.commits.iter().map(|(c, _)| c))
            .collect();
        let known = |oid: &Oid| candidates.contains(oid) || map.get(oid).is_some();
        self.refs.iter().all(|r| {
            r.commit.is_none_or(|head| known(&head)) && r.commits.iter().all(|(_, parents)| parents.iter().all(known))
        })
    }
}

/// The plan for `mirror` over the private repo at `git_dir`. `known` are the
/// private ref values this mirror's epoch already published; empty for a
/// rewrite from the root.
///
/// # Errors
///
/// Fails when the private repo can't be read.
pub(super) fn plan(git_dir: &Path, mirror: &PublishMirror, known: &[Oid]) -> Result<Plan> {
    let mut reader = ObjectReader::open(git_dir)?;
    let selected = selected_refs(git_dir, mirror)?;
    let mut excludes = BTreeSet::new();
    let mut found_file = false;
    let mut exclude_submodules = false;
    let mut strip_empty_dirs = false;
    let mut errors = Vec::new();
    for r in &selected {
        let Some(commit) = r.commit else { continue };
        let head = parse_commit(&reader.read_kind(&commit, ObjectKind::Commit)?)?;
        let (file, error) = load_repo_filter(&mut reader, head.tree)?;
        if let Some(error) = error {
            errors.push(format!("{}: {error}", r.name));
        }
        if let Some(file) = file {
            found_file = true;
            excludes.extend(file.exclude);
            exclude_submodules |= file.exclude_submodules;
            strip_empty_dirs |= file.strip_empty_dirs;
        }
    }
    let joined = found_file.then(|| RepoFilterFile {
        exclude: excludes.into_iter().collect(),
        exclude_submodules,
        strip_empty_dirs,
    });
    let mut seen = HashSet::new();
    let mut refs = Vec::with_capacity(selected.len());
    for r in selected {
        let commits = match r.commit {
            Some(head) => commits_since(git_dir, &head, known)?
                .into_iter()
                .filter(|(commit, _)| seen.insert(*commit))
                .collect(),
            None => Vec::new(),
        };
        refs.push(RefPlan {
            name: r.name,
            object: r.object,
            commit: r.commit,
            commits,
        });
    }
    Ok(Plan {
        refs,
        filter: Filter::new(&mirror.filter, joined.as_ref()),
        repo_file_error: (!errors.is_empty()).then(|| errors.join("; ")),
        fingerprint: Fingerprint {
            config: config_fingerprint(mirror),
            repo_files: repo_files_fingerprint(joined.as_ref()),
        },
    })
}

/// One published ref after the rewrite.
pub(super) struct RefResult {
    pub name: String,
    pub private_object: Oid,
    pub private_commit: Option<Oid>,
    /// What the public ref will hold: the rewritten commit, or the rewritten
    /// tag object for an annotated tag. `None` when [`Self::error`] says why.
    pub public: Option<Oid>,
    pub public_commit: Option<Oid>,
    pub public_tree: Option<Oid>,
    pub tag: Option<PublicTag>,
    /// The commits this ref brought to the public side in this run, as
    /// indices into the rewriter's `published`.
    pub published: Range<usize>,
    /// Blobs first met at an excluded path while rewriting this ref.
    pub newly_excluded: Vec<Oid>,
    /// Commits this ref had rewritten in this run, and how many of those
    /// collapsed away.
    pub rewritten: usize,
    pub collapsed: usize,
    pub published_files: usize,
    pub excluded_entries: usize,
    pub error: Option<String>,
}

pub(super) struct Rewrite {
    pub rewriter: Rewriter,
    pub refs: Vec<RefResult>,
}

/// Rewrites everything `plan` lists, seeding the rewriter from `map` so that
/// a commit an earlier run of the epoch already rewrote is never rewritten
/// again. `map` must hold every shard [`Plan::shards`] names.
///
/// # Errors
///
/// Fails when the private repo can't be read; a problem with one ref is
/// that ref's [`RefResult::error`].
pub(super) fn rewrite(git_dir: &Path, plan: &Plan, identities: &[IdentityMapping], map: &Table<40>) -> Result<Rewrite> {
    let mut reader = ObjectReader::open(git_dir)?;
    let mut rewriter = Rewriter::new(plan.filter.clone(), identities.to_vec());
    let seed = |rewriter: &mut Rewriter, oid: &Oid| {
        if rewriter.mapped(oid).is_none()
            && let Some(value) = map.get(oid)
        {
            rewriter.seed(*oid, from_map_value(value));
        }
    };
    let mut excluded_seen = HashSet::new();
    let mut refs = Vec::with_capacity(plan.refs.len());
    for r in &plan.refs {
        let start = rewriter.published.len();
        let (fresh, collapsed) = (rewriter.fresh.len(), rewriter.collapsed);
        for (commit, parents) in &r.commits {
            seed(&mut rewriter, commit);
            for parent in parents {
                seed(&mut rewriter, parent);
            }
            rewriter.rewrite_commit(&mut reader, *commit)?;
        }
        let mut result = RefResult {
            name: r.name.clone(),
            private_object: r.object,
            private_commit: r.commit,
            public: None,
            public_commit: None,
            public_tree: None,
            tag: None,
            published: start..rewriter.published.len(),
            newly_excluded: Vec::new(),
            rewritten: rewriter.fresh.len() - fresh,
            collapsed: rewriter.collapsed - collapsed,
            published_files: 0,
            excluded_entries: 0,
            error: None,
        };
        if let Err(e) = finish_ref(&mut reader, &mut rewriter, r, &mut result, &seed) {
            result.error = Some(format!("{e:#}"));
        }
        result.newly_excluded = rewriter
            .excluded_blobs
            .keys()
            .filter(|oid| excluded_seen.insert(**oid))
            .copied()
            .collect();
        refs.push(result);
    }
    Ok(Rewrite { rewriter, refs })
}

/// The ref's public value, its tag if annotated, and the head counts.
fn finish_ref(
    reader: &mut ObjectReader,
    rewriter: &mut Rewriter,
    r: &RefPlan,
    out: &mut RefResult,
    seed: &impl Fn(&mut Rewriter, &Oid),
) -> Result<()> {
    let Some(head) = r.commit else {
        anyhow::bail!("doesn't point at a commit");
    };
    seed(rewriter, &head);
    let (commit, tree) = match rewriter.public_of(&head) {
        Some(Some(public)) => public,
        Some(None) => anyhow::bail!("the filter leaves nothing to publish"),
        None => anyhow::bail!("{} was never rewritten", to_hex(&head)),
    };
    out.public_commit = Some(commit);
    out.public_tree = Some(tree);
    out.public = Some(commit);
    if r.object != head {
        let tag = tags::rewrite_tag(reader, rewriter, &r.object, commit)?;
        out.public = Some(tag.oid);
        out.tag = Some(tag);
    }
    // The head's tree is walked again, from the rewrite's cache when this
    // run rewrote the head, so the license gate and the counts see it even
    // when the head was only seeded from the mapping.
    let private_tree = parse_commit(&reader.read_kind(&head, ObjectKind::Commit)?)?.tree;
    // Also a check on the mapping: a seeded head whose tree no longer comes
    // out the same was rewritten under other inputs, and building on it
    // would publish a history that mixes two filters.
    if rewriter.rewrite_root(reader, private_tree)?.unwrap_or(EMPTY_TREE) != tree {
        anyhow::bail!(
            "{} maps to public tree {}, which this filter no longer produces; the epoch needs a bump",
            to_hex(&head),
            to_hex(&tree)
        );
    }
    let mut excluded = BTreeMap::new();
    count_exclusions(
        reader,
        rewriter.filter(),
        String::new(),
        private_tree,
        false,
        &mut excluded,
    )?;
    out.excluded_entries = excluded.values().sum();
    out.published_files = published_paths(rewriter, tree).len();
    Ok(())
}

/// The mapping entries this run adds: every commit it rewrote.
pub(super) fn fresh_entries(rewriter: &Rewriter) -> BTreeMap<Oid, MapValue> {
    rewriter
        .fresh
        .iter()
        .map(|private| {
            (
                *private,
                super::mapping::map_value(rewriter.public_of(private).flatten()),
            )
        })
        .collect()
}
