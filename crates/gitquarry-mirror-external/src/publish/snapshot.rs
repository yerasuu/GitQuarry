//! The local half of a snapshot run (`docs/publish-mirror-concept.md` §9,
//! phase 1): for each published branch, the filtered tree of its head and
//! the one commit that publishes it, written into a scratch repository and
//! checked there. Synchronous, like [`super::preview`], and for the same
//! reason; [`super::run`] calls it on a blocking thread.
//!
//! The scratch repository is a fresh `git init` that only ever receives
//! what this module writes: blobs read one by one from the private repo
//! because the public tree names them, the public trees, and the snapshot
//! commit. It is never a clone of the private repo and has no alternates,
//! so an excluded blob can't be in it, not even unreferenced — and
//! [`verify_scratch`] checks exactly that before anything is pushed.

use super::audit::{self, GateReport};
use super::filter::Filter;
use super::log::{PublicHead, PublishLog};
use super::objects::{
    Commit, LocalRef, ObjectKind, ObjectReader, Oid, encode_commit, encode_tree, from_hex, hash_object, parse_commit,
    split_ident, to_hex,
};
use super::preview::{count_exclusions, load_repo_filter, published_paths, selected_refs};
use super::rewrite::{EMPTY_TREE, Rewriter};
use super::scratch::{init_scratch, write_objects, written_by};
use super::{NEVER_PUBLISHED_REF_PREFIXES, PublishMirror, Publisher};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// What a published branch's snapshot came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Snapshot {
    /// The filtered tree is the one the previous snapshot already holds.
    Unchanged { commit: Oid, tree: Oid },
    /// A new commit on top of `parent` (none for the first publish).
    New {
        commit: Oid,
        tree: Oid,
        parent: Option<Oid>,
    },
}

/// One published branch after the local half of the run.
pub(super) struct RefBuild {
    pub ref_name: String,
    pub private_head: Oid,
    /// What the public branch held when the run started.
    pub remote_head: Option<String>,
    pub published_files: usize,
    pub excluded_entries: usize,
    pub gates: Vec<GateReport>,
    /// `None` when [`Self::error`] says why there is none.
    pub snapshot: Option<Snapshot>,
    pub error: Option<RefError>,
}

impl RefBuild {
    pub(super) fn gates_passed(&self) -> bool {
        self.gates.iter().all(GateReport::passed)
    }
}

pub(super) enum RefError {
    /// The public branch is somewhere this mirror never pushed it: an edit
    /// on the public side, or a first publish into a branch that already
    /// has commits. Building on it would adopt history nobody vetted, and
    /// replacing it would be a force push; this mode does neither.
    Foreign {
        remote_head: String,
    },
    Other(String),
}

impl std::fmt::Display for RefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefError::Foreign { remote_head } => write!(
                f,
                "the public branch is at {remote_head}, which this mirror didn't publish; not building on it \
                 and not forcing over it"
            ),
            RefError::Other(error) => f.write_str(error),
        }
    }
}

/// Every branch the run publishes, and what the scratch repo must hold.
pub(super) struct Build {
    pub refs: Vec<RefBuild>,
    /// Every object written into the scratch repo.
    written: HashSet<Oid>,
    /// Every blob met at an excluded path, in any published branch's history.
    excluded: HashSet<Oid>,
}

/// The local half of a run: plans, writes and gates every published branch.
/// `remote_refs` is the public side as `ls-remote` saw it; `log` is what
/// this mirror last pushed there.
///
/// # Errors
///
/// Fails when the private repo or the scratch repo can't be read or
/// written; a problem with one branch is that branch's [`RefBuild::error`].
pub(super) fn build(
    private_git_dir: &Path,
    scratch: &Path,
    mirror: &PublishMirror,
    remote_refs: &HashMap<String, String>,
    log: &PublishLog,
) -> Result<Build> {
    let repo = scratch.join("public.git");
    let stage = scratch.join("stage");
    init_scratch(&repo)?;
    std::fs::create_dir_all(&stage).with_context(|| format!("creating {}", stage.display()))?;
    let mut builder = Builder {
        reader: ObjectReader::open(private_git_dir)?,
        private_git_dir,
        repo: &repo,
        stage: &stage,
        mirror,
        written: HashSet::new(),
        excluded: HashSet::new(),
    };
    let mut refs = Vec::new();
    for LocalRef {
        name: ref_name,
        commit: head,
        ..
    } in selected_refs(private_git_dir, mirror)?
    {
        // Branches only: a snapshot is a new commit, and a tag that moves to
        // one is not a tag anybody can rely on.
        if !ref_name.starts_with("refs/heads/") || NEVER_PUBLISHED_REF_PREFIXES.iter().any(|p| ref_name.starts_with(p))
        {
            continue;
        }
        let remote_head = remote_refs.get(&ref_name).cloned();
        let mut ref_build = RefBuild {
            ref_name,
            private_head: head.unwrap_or_default(),
            remote_head,
            published_files: 0,
            excluded_entries: 0,
            gates: Vec::new(),
            snapshot: None,
            error: None,
        };
        let outcome = match (
            head,
            base_for(&ref_build.ref_name, ref_build.remote_head.as_deref(), log),
        ) {
            (None, _) => Err(anyhow::anyhow!("doesn't point at a commit")),
            (Some(_), Err(foreign)) => {
                ref_build.error = Some(foreign);
                Ok(())
            }
            (Some(head), Ok(base)) => builder.build_ref(&mut ref_build, head, base.as_ref()),
        };
        if let Err(e) = outcome {
            ref_build.error = Some(RefError::Other(format!("{e:#}")));
        }
        refs.push(ref_build);
    }
    Ok(Build {
        refs,
        written: builder.written,
        excluded: builder.excluded,
    })
}

/// Which public commit `ref_name` builds on. `Err` names the foreign head.
fn base_for(ref_name: &str, remote_head: Option<&str>, log: &PublishLog) -> Result<Option<PublicHead>, RefError> {
    let Some(remote_head) = remote_head else {
        return Ok(None);
    };
    log.heads
        .get(ref_name)
        .into_iter()
        .chain(log.pending.get(ref_name))
        .find(|known| known.commit == remote_head)
        .cloned()
        .map(Some)
        .ok_or_else(|| RefError::Foreign {
            remote_head: remote_head.to_string(),
        })
}

struct Builder<'a> {
    reader: ObjectReader,
    private_git_dir: &'a Path,
    repo: &'a Path,
    stage: &'a Path,
    mirror: &'a PublishMirror,
    /// Every id [`write_objects`] has confirmed in the scratch repo.
    written: HashSet<Oid>,
    /// Every blob met at an excluded path, in any published history.
    excluded: HashSet<Oid>,
}

impl Builder<'_> {
    /// Fills `out` for one branch building on `base`.
    fn build_ref(&mut self, out: &mut RefBuild, head: Oid, base: Option<&PublicHead>) -> Result<()> {
        let mirror = self.mirror;
        let head_commit = parse_commit(&self.reader.read_kind(&head, ObjectKind::Commit)?)?;
        let (repo_file, repo_file_error) = load_repo_filter(&mut self.reader, head_commit.tree)?;
        let filter = Filter::new(&mirror.filter, repo_file.as_ref());
        let mut rewriter = Rewriter::new(filter.clone(), mirror.identities.clone());
        // The whole history, not just the head: the blob-set audit has to
        // know every blob that ever sat at an excluded path, so that content
        // moved out of `deploy/` into a public path is caught (note #46).
        for commit in super::objects::topo_order(self.private_git_dir, &head)? {
            rewriter.rewrite_commit(&mut self.reader, commit)?;
        }
        self.excluded.extend(rewriter.excluded_blobs.keys().copied());
        let tree = rewriter
            .mapped(&head)
            .flatten()
            .and_then(|c| rewriter.public_tree_of(&c))
            .filter(|tree| *tree != EMPTY_TREE)
            .context("the filter leaves nothing to publish")?;
        let mut excluded = BTreeMap::new();
        count_exclusions(
            &mut self.reader,
            &filter,
            String::new(),
            head_commit.tree,
            false,
            &mut excluded,
        )?;
        out.excluded_entries = excluded.values().sum();
        out.published_files = published_paths(&rewriter, tree).len();

        let planned = plan_snapshot(mirror.publisher.as_ref(), &out.ref_name, &head_commit, tree, base)?;
        if matches!(planned.snapshot, Some(Snapshot::Unchanged { .. })) {
            out.snapshot = planned.snapshot;
            return Ok(());
        }
        let message = snapshot_message(&out.ref_name);
        self.write_tree_objects(&rewriter, tree)?;
        if let (Some(Snapshot::New { commit, .. }), Some(body)) = (planned.snapshot, planned.commit_body) {
            self.write(ObjectKind::Commit, [(commit, body)])?;
        }

        // Against the scratch repo: the gates read the objects actually
        // written, whose ids `write_objects` has just checked against the
        // ones computed here.
        let mut written = ObjectReader::open(self.repo)?;
        let set = audit::published_set_from([tree], &rewriter.public_trees, &filter);
        out.gates = vec![
            audit::filter_gate(&set, repo_file_error),
            audit::blob_set_gate(&set, &rewriter),
            audit::license_gate(&mut written, &rewriter, Some(tree), mirror.spdx.as_deref())?,
            audit::secret_gate(
                &mut written,
                &set,
                [(head, message.as_bytes())],
                &mirror.secret_allowlist,
            )?,
            audit::publisher_gate(mirror.publisher.as_ref()),
        ];
        out.snapshot = planned.snapshot;
        Ok(())
    }

    /// Writes every tree under `root` and every blob they name. Blobs are
    /// read from the private repo only because a public tree names them, so
    /// nothing the filter dropped is ever read, let alone written.
    fn write_tree_objects(&mut self, rewriter: &Rewriter, root: Oid) -> Result<()> {
        let mut trees = Vec::new();
        let mut blobs = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        while let Some(tree) = stack.pop() {
            if !seen.insert(tree) {
                continue;
            }
            let entries = rewriter
                .public_trees
                .get(&tree)
                .with_context(|| format!("public tree {} was never computed", to_hex(&tree)))?;
            for entry in entries {
                if entry.is_tree() {
                    stack.push(entry.oid);
                } else if !entry.is_gitlink() && seen.insert(entry.oid) {
                    blobs.push(entry.oid);
                }
            }
            trees.push((tree, encode_tree(entries)));
        }
        let mut bodies = Vec::with_capacity(blobs.len());
        for blob in blobs {
            bodies.push((blob, self.reader.read_kind(&blob, ObjectKind::Blob)?));
        }
        self.write(ObjectKind::Blob, bodies)?;
        self.write(ObjectKind::Tree, trees)
    }

    fn write(&mut self, kind: ObjectKind, objects: impl IntoIterator<Item = (Oid, Vec<u8>)>) -> Result<()> {
        let ids = write_objects(self.repo, self.stage, kind, objects)?;
        self.written.extend(ids);
        Ok(())
    }
}

/// A branch's snapshot decided in memory, before anything is written.
pub(super) struct PlannedSnapshot {
    /// `None` when there is no publisher to author a new commit.
    pub snapshot: Option<Snapshot>,
    /// The body that hashes to the `New` commit.
    pub commit_body: Option<Vec<u8>>,
}

/// What a publish would push for `tree` on top of `base`. Shared by the run
/// and the dry-run preview so the sha the preview shows is the one the run
/// pushes: neither side has its own copy of the commit's inputs.
pub(super) fn plan_snapshot(
    publisher: Option<&Publisher>,
    ref_name: &str,
    private_head: &Commit,
    tree: Oid,
    base: Option<&PublicHead>,
) -> Result<PlannedSnapshot> {
    let parent = match base {
        Some(base) if from_hex(&base.tree)? == tree => {
            return Ok(PlannedSnapshot {
                snapshot: Some(Snapshot::Unchanged {
                    commit: from_hex(&base.commit)?,
                    tree,
                }),
                commit_body: None,
            });
        }
        Some(base) => Some(from_hex(&base.commit)?),
        None => None,
    };
    let Some(publisher) = publisher else {
        return Ok(PlannedSnapshot {
            snapshot: None,
            commit_body: None,
        });
    };
    let body = snapshot_commit(
        publisher,
        &private_head.committer,
        tree,
        parent,
        &snapshot_message(ref_name),
    )?;
    Ok(PlannedSnapshot {
        snapshot: Some(Snapshot::New {
            commit: hash_object(ObjectKind::Commit, &body),
            tree,
            parent,
        }),
        commit_body: Some(body),
    })
}

/// Fixed text: the private messages are exactly what a snapshot leaves
/// behind, and the branch name is already public as the target ref.
fn snapshot_message(ref_name: &str) -> String {
    let branch = ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name);
    format!("Publish snapshot of {branch}\n")
}

/// The snapshot commit. Deterministic in its inputs: the publisher as author
/// and committer, both dated with the private head's *committer* timestamp
/// and zone, so publishing the same source head on top of the same parent
/// always yields the same id — a run that pushed but couldn't record it
/// recognizes its own commit next time (see [`PublishLog::pending`]).
fn snapshot_commit(
    publisher: &Publisher,
    private_committer: &[u8],
    tree: Oid,
    parent: Option<Oid>,
    message: &str,
) -> Result<Vec<u8>> {
    let when = split_ident(private_committer)
        .map(|ident| ident.when.to_vec())
        .context("the private head's committer line has no date")?;
    let mut ident = format!("{} <{}> ", publisher.name, publisher.email).into_bytes();
    ident.extend_from_slice(&when);
    Ok(encode_commit(&Commit {
        tree,
        parents: parent.into_iter().collect(),
        author: ident.clone(),
        committer: ident,
        other_headers: Vec::new(),
        message: message.as_bytes().to_vec(),
    }))
}

/// The last check before the push: the scratch repo holds exactly what
/// [`build`] wrote and none of the blobs any published history excluded.
///
/// # Errors
///
/// Fails on any object nobody wrote, or any excluded blob present.
pub(super) fn verify_scratch(scratch: &Path, build: &Build) -> Result<()> {
    let present = written_by(&scratch.join("public.git"))?;
    if let Some(stray) = present.difference(&build.written).next() {
        bail!(
            "the scratch repo holds object {} that the snapshot didn't write",
            to_hex(stray)
        );
    }
    if let Some(leak) = present.intersection(&build.excluded).next() {
        bail!(
            "the scratch repo holds blob {}, which the filter excludes",
            to_hex(leak)
        );
    }
    Ok(())
}
