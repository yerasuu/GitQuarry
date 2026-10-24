//! `rewrite_commit`/`rewrite_tree` from `docs/publish-mirror-concept.md` §3,
//! computed and never written: every public object id comes from hashing
//! the object git *would* store, so phase 0 produces the real public head
//! sha without a single object existing anywhere on the public side. No blob
//! is ever re-encoded at all — a kept blob keeps its id, and an excluded one
//! is only ever seen as an id inside its parent tree.

use super::IdentityMapping;
use super::filter::{Decision, EntryKind, Filter};
use super::objects::{
    Commit, ObjectKind, ObjectReader, Oid, TreeEntry, encode_commit, encode_tree, hash_object, parse_commit,
    parse_tree, split_ident, to_hex,
};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};

/// Whose identity on a published commit.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IdentRole {
    Author,
    Committer,
    /// The tagger of an annotated tag.
    Tagger,
}

/// A private commit that appears on the public side.
#[derive(Debug)]
pub(crate) struct PublishedCommit {
    pub private: Oid,
    pub public: Oid,
    pub tree: Oid,
    /// The public commit object, exactly as hashed into [`Self::public`].
    pub body: Vec<u8>,
    message_at: usize,
    /// Emails the identity map didn't cover, passed through as they were.
    pub unmapped: Vec<(IdentRole, String)>,
}

impl PublishedCommit {
    pub(crate) fn message(&self) -> &[u8] {
        &self.body[self.message_at..]
    }
}

/// One history rewrite under one filter. Memoized per private commit and per
/// (path, private tree): the cache can't be keyed by tree id alone, as the
/// concept document sketches, because an anchored pattern (`/deploy/**`)
/// excludes a subtree at one path and not at another.
///
/// Owns its filter so that a history run can carry it across the awaits
/// between its blocking steps.
pub(crate) struct Rewriter {
    filter: Filter,
    identities: Vec<IdentityMapping>,
    tree_cache: HashMap<(String, Oid, bool), Option<Oid>>,
    mapping: HashMap<Oid, Option<Oid>>,
    commit_trees: HashMap<Oid, Oid>,
    /// Private commits rewritten here, as opposed to [`Self::seed`]ed from
    /// an earlier run, in the order they were rewritten.
    pub fresh: Vec<Oid>,
    /// Every public tree produced, by public id, with its entries.
    pub public_trees: HashMap<Oid, Vec<TreeEntry>>,
    /// Blobs met at an excluded path, with the first such path.
    pub excluded_blobs: HashMap<Oid, String>,
    excluded_trees_seen: HashSet<Oid>,
    pub published: Vec<PublishedCommit>,
    pub collapsed: usize,
    /// Parents never rewritten count as the root of the history instead of
    /// an error, for a window that cuts the history short.
    windowed: bool,
}

/// git's empty tree, used for a commit whose whole tree was filtered away
/// but which still has to exist (its parent's tree differs), and for an
/// emptied directory kept because `strip_empty_dirs` is off.
pub(crate) const EMPTY_TREE: Oid = [
    0x4b, 0x82, 0x5d, 0xc6, 0x42, 0xcb, 0x6e, 0xb9, 0xa0, 0x60, 0xe5, 0x4b, 0xf8, 0xd6, 0x92, 0x88, 0xfb, 0xee, 0x49,
    0x04,
];

impl Rewriter {
    pub(crate) fn new(filter: Filter, identities: Vec<IdentityMapping>) -> Self {
        Self {
            filter,
            identities,
            tree_cache: HashMap::new(),
            mapping: HashMap::new(),
            commit_trees: HashMap::new(),
            fresh: Vec::new(),
            public_trees: HashMap::new(),
            excluded_blobs: HashMap::new(),
            excluded_trees_seen: HashSet::new(),
            published: Vec::new(),
            collapsed: 0,
            windowed: false,
        }
    }

    /// For rewriting only the newest commits of a history: the ones whose
    /// parents fall outside the window become roots, so everything computed
    /// afterwards describes the window and not the whole history.
    pub(crate) fn windowed(mut self) -> Self {
        self.windowed = true;
        self
    }

    /// The public commit `private` maps to: `None` if it hasn't been
    /// rewritten, `Some(None)` if it and all its ancestors collapsed away.
    pub(crate) fn mapped(&self, private: &Oid) -> Option<Option<Oid>> {
        self.mapping.get(private).copied()
    }

    pub(crate) fn public_tree_of(&self, public_commit: &Oid) -> Option<Oid> {
        self.commit_trees.get(public_commit).copied()
    }

    pub(crate) fn filter(&self) -> &Filter {
        &self.filter
    }

    /// Records what an earlier run of the same epoch rewrote `private` to:
    /// the public commit and its tree, or `None` when it and its ancestors
    /// collapsed away. A seeded commit is never rewritten again, which is
    /// what makes a run after a push rewrite only the new commits.
    pub(crate) fn seed(&mut self, private: Oid, public: Option<(Oid, Oid)>) {
        self.mapping.insert(private, public.map(|(commit, _)| commit));
        if let Some((commit, tree)) = public {
            self.commit_trees.insert(commit, tree);
        }
    }

    /// `private`'s public commit and that commit's tree, once rewritten or
    /// seeded: `Some(None)` when it collapsed away entirely.
    pub(crate) fn public_of(&self, private: &Oid) -> Option<Option<(Oid, Oid)>> {
        match *self.mapping.get(private)? {
            None => Some(None),
            Some(public) => self.commit_trees.get(&public).map(|tree| Some((public, *tree))),
        }
    }

    /// The public tree of the private root tree `private`, computed (or
    /// taken from the cache) so that [`Self::public_trees`] holds all of it.
    pub(crate) fn rewrite_root(&mut self, reader: &mut ObjectReader, private: Oid) -> Result<Option<Oid>> {
        self.rewrite_tree(reader, String::new(), private, false)
    }

    /// Rewrites `private`. Its parents must have been rewritten first, which
    /// `objects::topo_order` guarantees.
    pub(crate) fn rewrite_commit(&mut self, reader: &mut ObjectReader, private: Oid) -> Result<()> {
        if self.mapping.contains_key(&private) {
            return Ok(());
        }
        let commit = parse_commit(&reader.read_kind(&private, ObjectKind::Commit)?)
            .with_context(|| format!("parsing commit {}", to_hex(&private)))?;
        let mut parents: Vec<Oid> = Vec::with_capacity(commit.parents.len());
        for parent in &commit.parents {
            let mapped = match self.mapping.get(parent) {
                Some(mapped) => mapped,
                None if self.windowed => &None,
                None => {
                    return Err(anyhow::anyhow!(
                        "parent {} of {} not rewritten yet",
                        to_hex(parent),
                        to_hex(&private)
                    ));
                }
            };
            if let Some(mapped) = mapped
                && !parents.contains(mapped)
            {
                parents.push(*mapped);
            }
        }
        let tree = self.rewrite_tree(reader, String::new(), commit.tree, false)?;
        if parents.len() <= 1 {
            let parent_tree = parents.first().and_then(|p| self.commit_trees.get(p));
            let collapses = match (tree, parent_tree) {
                (None, None) => true,
                (None, Some(parent_tree)) => *parent_tree == EMPTY_TREE,
                (Some(tree), Some(parent_tree)) => tree == *parent_tree,
                (Some(_), None) => false,
            };
            if collapses {
                self.mapping.insert(private, parents.first().copied());
                self.fresh.push(private);
                self.collapsed += 1;
                return Ok(());
            }
        }
        let tree = tree.unwrap_or_else(|| {
            self.public_trees.entry(EMPTY_TREE).or_default();
            EMPTY_TREE
        });
        let mut unmapped = Vec::new();
        let author = self.map_ident(&commit.author, IdentRole::Author, &mut unmapped);
        let committer = self.map_ident(&commit.committer, IdentRole::Committer, &mut unmapped);
        let rewritten = Commit {
            tree,
            parents,
            author,
            committer,
            other_headers: commit.other_headers,
            message: commit.message,
        };
        let body = encode_commit(&rewritten);
        let public = hash_object(ObjectKind::Commit, &body);
        self.mapping.insert(private, Some(public));
        self.fresh.push(private);
        self.commit_trees.insert(public, tree);
        self.published.push(PublishedCommit {
            private,
            public,
            tree,
            message_at: body.len() - rewritten.message.len(),
            body,
            unmapped,
        });
        Ok(())
    }

    /// Author and committer dates pass through verbatim: they are part of
    /// what makes the rewrite deterministic.
    pub(crate) fn map_ident(&self, value: &[u8], role: IdentRole, unmapped: &mut Vec<(IdentRole, String)>) -> Vec<u8> {
        let Some(ident) = split_ident(value) else {
            unmapped.push((role, String::from_utf8_lossy(value).into_owned()));
            return value.to_vec();
        };
        let email = String::from_utf8_lossy(ident.email);
        let Some(mapping) = self.identities.iter().find(|m| m.private.eq_ignore_ascii_case(&email)) else {
            unmapped.push((role, email.into_owned()));
            return value.to_vec();
        };
        let name = mapping.name.as_deref().map_or(ident.name, str::as_bytes);
        let mut out = Vec::with_capacity(value.len() + mapping.public.len());
        out.extend_from_slice(name);
        out.extend_from_slice(b" <");
        out.extend_from_slice(mapping.public.as_bytes());
        out.extend_from_slice(b"> ");
        out.extend_from_slice(ident.when);
        out
    }

    /// `None` when nothing in the tree survives the filter.
    fn rewrite_tree(
        &mut self,
        reader: &mut ObjectReader,
        path: String,
        private: Oid,
        included: bool,
    ) -> Result<Option<Oid>> {
        let key = (path, private, included);
        if let Some(cached) = self.tree_cache.get(&key) {
            return Ok(*cached);
        }
        let path = &key.0;
        let entries = parse_tree(&reader.read_kind(&private, ObjectKind::Tree)?)
            .with_context(|| format!("parsing tree {}", to_hex(&private)))?;
        let mut kept = Vec::with_capacity(entries.len());
        for entry in entries {
            let child = join(path, &entry.name_lossy());
            match self.filter.decide(&child, entry_kind(&entry), included) {
                Decision::Exclude(_) => self.record_excluded(reader, child, &entry)?,
                Decision::Keep { included } if entry.is_tree() => {
                    match self.rewrite_tree(reader, child, entry.oid, included)? {
                        Some(oid) => kept.push(TreeEntry { oid, ..entry }),
                        None if !self.filter.strip_empty_dirs() => {
                            self.public_trees.entry(EMPTY_TREE).or_default();
                            kept.push(TreeEntry {
                                oid: EMPTY_TREE,
                                ..entry
                            });
                        }
                        None => {}
                    }
                }
                Decision::Keep { .. } => kept.push(entry),
            }
        }
        let result = if kept.is_empty() {
            None
        } else {
            let oid = hash_object(ObjectKind::Tree, &encode_tree(&kept));
            self.public_trees.insert(oid, kept);
            Some(oid)
        };
        self.tree_cache.insert(key, result);
        Ok(result)
    }

    /// Remembers every blob under an excluded entry, for the blob-set audit.
    /// Reads tree objects only; the blobs themselves are never loaded.
    fn record_excluded(&mut self, reader: &mut ObjectReader, path: String, entry: &TreeEntry) -> Result<()> {
        if entry.is_gitlink() {
            return Ok(());
        }
        if !entry.is_tree() {
            self.excluded_blobs.entry(entry.oid).or_insert(path);
            return Ok(());
        }
        if !self.excluded_trees_seen.insert(entry.oid) {
            return Ok(());
        }
        let entries = parse_tree(&reader.read_kind(&entry.oid, ObjectKind::Tree)?)
            .with_context(|| format!("parsing tree {}", to_hex(&entry.oid)))?;
        for child in &entries {
            let child_path = join(&path, &child.name_lossy());
            self.record_excluded(reader, child_path, child)?;
        }
        Ok(())
    }
}

pub(crate) fn entry_kind(entry: &TreeEntry) -> EntryKind {
    if entry.is_tree() {
        EntryKind::Dir
    } else if entry.is_gitlink() {
        EntryKind::Gitlink
    } else {
        EntryKind::File
    }
}

pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}
