//! The local, writing half of a history run: which public objects are new,
//! writing exactly those into the publish repo, and every gate over what is
//! about to be pushed. Synchronous; [`super::run`] calls it on a blocking
//! thread, after the bucket reads it needs.

use super::Rewrite;
use crate::publish::PublishMirror;
use crate::publish::audit::{self, GateReport, PublishedSet};
use crate::publish::mapping::Table;
use crate::publish::objects::{EMPTY_BLOB, ObjectKind, ObjectReader, Oid, encode_tree, to_hex};
use crate::publish::scratch::{Presence, loose_objects, write_objects};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// What this run adds to the publish repo.
pub(super) struct Outgoing {
    trees: Vec<Oid>,
    pub blobs: Vec<Oid>,
    /// Blobs this run first met at an excluded path that the publish repo
    /// already holds: content that went public before it was excluded.
    already_public: HashSet<Oid>,
}

/// The trees and blobs under every newly published commit and under the
/// head of every ref in `pushing` that the publish repo doesn't hold yet. A
/// tree the repo holds is skipped whole: an earlier run stored it with all
/// it names.
///
/// # Errors
///
/// Fails when the publish repo can't be read, or a public tree the rewrite
/// should have computed is missing (a bug, never published around).
pub(super) fn outgoing(repo: &Path, rewrite: &Rewrite, pushing: &[usize]) -> Result<Outgoing> {
    let rewriter = &rewrite.rewriter;
    let mut presence = Presence::open(repo)?;
    let mut stack: Vec<Oid> = rewriter.published.iter().map(|c| c.tree).collect();
    stack.extend(pushing.iter().filter_map(|i| rewrite.refs[*i].public_tree));
    let mut seen = HashSet::new();
    let mut out = Outgoing {
        trees: Vec::new(),
        blobs: Vec::new(),
        already_public: HashSet::new(),
    };
    while let Some(tree) = stack.pop() {
        if !seen.insert(tree) || presence.contains(&tree)? {
            continue;
        }
        let entries = rewriter
            .public_trees
            .get(&tree)
            .with_context(|| format!("public tree {} was never computed", to_hex(&tree)))?;
        for entry in entries {
            if entry.is_tree() {
                stack.push(entry.oid);
            } else if !entry.is_gitlink() && seen.insert(entry.oid) && !presence.contains(&entry.oid)? {
                out.blobs.push(entry.oid);
            }
        }
        out.trees.push(tree);
    }
    for oid in rewriter.excluded_blobs.keys() {
        if *oid != EMPTY_BLOB && presence.contains(oid)? {
            out.already_public.insert(*oid);
        }
    }
    Ok(out)
}

/// Where a history run reads and writes.
#[derive(Clone, Copy)]
pub(super) struct Paths<'a> {
    pub private_git_dir: &'a Path,
    /// The publish repo.
    pub repo: &'a Path,
    /// Where objects are staged on their way into it.
    pub stage: &'a Path,
}

/// Each pushed ref's gates, and what the run wrote.
pub(super) struct Built {
    pub gates: Vec<Vec<GateReport>>,
    /// Every object this run wrote into the publish repo.
    pub written: Vec<Oid>,
}

/// Writes `outgoing`, every newly published commit and the tags of the
/// refs in `pushing` into `repo`, then gates each ref in `pushing` on what
/// was written. `excluded` is the epoch's stored excluded-blob set, loaded
/// for at least the shards of every outgoing blob.
///
/// # Errors
///
/// Fails when either repo can't be read or written; gate findings are not
/// errors, they are in [`Built::gates`].
pub(super) fn write_and_gate(
    paths: &Paths<'_>,
    rewrite: &Rewrite,
    outgoing: &Outgoing,
    pushing: &[usize],
    excluded: &Table<0>,
    mirror: &PublishMirror,
    repo_file_error: Option<&str>,
) -> Result<Built> {
    let Paths {
        private_git_dir,
        repo,
        stage,
    } = *paths;
    let rewriter = &rewrite.rewriter;
    let mut private = ObjectReader::open(private_git_dir)?;
    let mut written = Vec::new();
    // Blobs are read from the private repo only because a public tree names
    // them, so nothing the filter dropped is ever read, let alone written.
    let mut blobs = Vec::with_capacity(outgoing.blobs.len());
    for blob in &outgoing.blobs {
        blobs.push((*blob, private.read_kind(blob, ObjectKind::Blob)?));
    }
    written.extend(write_objects(repo, stage, ObjectKind::Blob, blobs)?);
    let trees = outgoing
        .trees
        .iter()
        .map(|tree| {
            let entries = rewriter
                .public_trees
                .get(tree)
                .with_context(|| format!("public tree {} was never computed", to_hex(tree)))?;
            Ok((*tree, encode_tree(entries)))
        })
        .collect::<Result<Vec<_>>>()?;
    written.extend(write_objects(repo, stage, ObjectKind::Tree, trees)?);
    let commits = rewriter.published.iter().map(|c| (c.public, c.body.clone()));
    written.extend(write_objects(repo, stage, ObjectKind::Commit, commits)?);
    let tags = pushing
        .iter()
        .filter_map(|i| rewrite.refs[*i].tag.as_ref())
        .map(|tag| (tag.oid, tag.body.clone()));
    written.extend(write_objects(repo, stage, ObjectKind::Tag, tags)?);

    let new_blobs: HashSet<&Oid> = outgoing.blobs.iter().collect();
    let mut reader = ObjectReader::open(repo)?;
    let mut gates = Vec::with_capacity(pushing.len());
    for i in pushing {
        let r = &rewrite.refs[*i];
        let commits = &rewriter.published[r.published.clone()];
        let roots = commits.iter().map(|c| c.tree).chain(r.public_tree);
        let set = audit::published_set_from(roots, &rewriter.public_trees, rewriter.filter());
        let pairs = set
            .blobs
            .iter()
            .filter_map(|(oid, published)| {
                let excluded_path = match rewriter.excluded_blobs.get(oid) {
                    Some(path) => path.clone(),
                    None if excluded.get(oid).is_some() => String::new(),
                    None => return None,
                };
                Some((*oid, published.clone(), excluded_path))
            })
            .chain(r.newly_excluded.iter().filter_map(|oid| {
                let excluded_path = rewriter.excluded_blobs.get(oid)?;
                outgoing
                    .already_public
                    .contains(oid)
                    .then(|| (*oid, String::new(), excluded_path.clone()))
            }));
        let blob_set = audit::blob_set_gate_from(pairs);
        // The secret scan reads what is new: a blob already in the publish
        // repo was scanned by the run that stored it.
        let scanned = PublishedSet {
            blobs: set
                .blobs
                .iter()
                .filter(|(oid, _)| new_blobs.contains(oid))
                .map(|(oid, path)| (*oid, path.clone()))
                .collect(),
            filter_findings: Vec::new(),
        };
        let messages = commits
            .iter()
            .map(|c| (c.private, c.message()))
            .chain(r.tag.as_ref().map(|tag| (r.private_object, tag.message())));
        let identities = commits
            .iter()
            .map(|c| (c.private, c.unmapped.as_slice()))
            .chain(r.tag.as_ref().map(|tag| (r.private_object, tag.unmapped.as_slice())));
        gates.push(vec![
            audit::filter_gate(&set, repo_file_error.map(str::to_string)),
            blob_set,
            audit::license_gate(&mut reader, rewriter, r.public_tree, mirror.spdx.as_deref())?,
            audit::secret_gate(&mut reader, &scanned, messages, &mirror.secret_allowlist)?,
            audit::identity_gate_over(identities, mirror.unmapped),
        ]);
    }
    Ok(Built { gates, written })
}

/// The last check before anything leaves: every loose object in the publish
/// repo is one this run wrote, and none of them is a blob the filter
/// excluded anywhere in the history this run or the epoch's stored set know.
///
/// # Errors
///
/// Fails on any object nobody wrote, or any excluded blob present.
pub(super) fn verify(repo: &Path, built: &Built, rewrite: &Rewrite, excluded: &Table<0>) -> Result<()> {
    let written: HashSet<&Oid> = built.written.iter().collect();
    for oid in loose_objects(repo)? {
        if !written.contains(&oid) {
            bail!(
                "the publish repo holds object {} that this run didn't write",
                to_hex(&oid)
            );
        }
        if rewrite.rewriter.excluded_blobs.contains_key(&oid) || excluded.get(&oid).is_some() {
            bail!(
                "the publish repo holds blob {}, which the filter excludes",
                to_hex(&oid)
            );
        }
    }
    Ok(())
}

/// Blob first bytes whose excluded-set shards the gates need.
pub(super) fn excluded_shards(outgoing: &Outgoing) -> impl Iterator<Item = u8> + '_ {
    outgoing.blobs.iter().map(|b| b[0])
}

/// The excluded-set entries this run adds.
pub(super) fn fresh_excluded(rewrite: &Rewrite) -> BTreeMap<Oid, [u8; 0]> {
    rewrite.rewriter.excluded_blobs.keys().map(|oid| (*oid, [])).collect()
}
