//! The translation of `docs/publish-mirror-inbound-concept.md` §3.3: each
//! public commit's change, path by path, overlaid on the private tree of the
//! commit before it, so that every entry the public side never saw (the
//! excluded files and directories) stays exactly where it was. The filtered
//! public tree is never turned back into a private one.

use super::quarantine::blob_sizes;
use super::{Cap, MAX_BLOB_BYTES, MAX_CHANGED_FILES, MAX_PR_BYTES, Reason, SecretFinding, Stop, refuse};
use crate::publish::IdentityMapping;
use crate::publish::filter::{EntryKind, Filter};
use crate::publish::objects::{
    Commit, EMPTY_BLOB, ObjectKind, ObjectReader, Oid, TreeEntry, encode_commit, encode_tree, parse_commit, parse_tree,
    to_hex,
};
use crate::publish::rewrite::{Rewriter, join};
use crate::publish::secrets::{Scan, scan_blob, scan_text};
use crate::validate::{MAX_INBOUND_PATH_DEPTH, validate_inbound_path};
use icu_normalizer::ComposingNormalizerBorrowed;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

const MODE_FILE: &[u8] = b"100644";
const MODE_EXECUTABLE: &[u8] = b"100755";
const MODE_SYMLINK: &[u8] = b"120000";
const MODE_GITLINK: &[u8] = b"160000";
const MODE_TREE: &[u8] = b"40000";

/// Names no inbound change may touch at any depth, compared without case
/// since a checkout on a case-insensitive filesystem reads them so: they
/// change how every other file is diffed, merged or checked out (§3.12).
const SENSITIVE_NAMES: [&str; 2] = [".gitattributes", ".gitmodules"];

/// Top-level entries that define CI or this server's own repo settings: a
/// change to one runs, or configures, something with the private repo's
/// privileges (§3.12, "CI triggers" and "`.gitquarry/` namespace"; Q7 of
/// kb #117). `.woodpecker` is matched as a prefix, for its `.yml` and
/// directory forms alike.
const SENSITIVE_TOP_LEVEL: [&str; 11] = [
    ".gitquarry",
    ".github",
    ".gitlab-ci.yml",
    ".gitea",
    ".forgejo",
    ".circleci",
    ".buildkite",
    ".travis.yml",
    "Jenkinsfile",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
];

pub(super) struct Input {
    pub private_repo: PathBuf,
    pub quarantine: PathBuf,
    pub filter: Filter,
    pub public_base: Oid,
    pub private_base: Oid,
    /// Linear, oldest first, each one's parent the one before it (the first
    /// one's [`Self::public_base`]).
    pub commits: Vec<Oid>,
}

pub(super) struct Translated {
    /// Over the private repo, holding the translated commits and their new
    /// trees in memory.
    pub reader: ObjectReader,
    /// `(public, private)`, oldest first.
    pub commits: Vec<(Oid, Oid)>,
    /// Every blob the pull request adds, with the first path it is added at.
    pub added: Vec<(Oid, String)>,
    pub secrets: Vec<SecretFinding>,
}

/// One file the public side changed: its entry before and after, `None`
/// where there was none.
struct Change {
    path: String,
    old: Option<Leaf>,
    new: Option<Leaf>,
}

#[derive(Debug, PartialEq, Eq)]
struct Leaf {
    mode: Vec<u8>,
    oid: Oid,
}

impl Leaf {
    fn of(entry: &TreeEntry) -> Self {
        Self {
            mode: entry.mode.clone(),
            oid: entry.oid,
        }
    }
}

/// The private commit, among `candidates` (every private commit the
/// inverse lists under the public base), that produced the public base by
/// itself: the one none of whose parents is a candidate. The others
/// collapsed onto it later (§3.2).
pub(super) fn originating(private_repo: &std::path::Path, candidates: &BTreeSet<Oid>) -> Result<Oid, Stop> {
    let mut reader = ObjectReader::open(private_repo)?;
    let mut roots = Vec::with_capacity(1);
    for candidate in candidates {
        let commit = parse_commit(&reader.read_kind(candidate, ObjectKind::Commit)?)?;
        if !commit.parents.iter().any(|parent| candidates.contains(parent)) {
            roots.push(*candidate);
        }
    }
    match roots.as_slice() {
        [root] => Ok(*root),
        [] => Err(anyhow::anyhow!("the inverse lists private commits that are each other's ancestors").into()),
        _ => Err(refuse(Reason::AmbiguousBase, None, None)),
    }
}

pub(super) fn translate(input: Input) -> Result<Translated, Stop> {
    let mut public = ObjectReader::open(&input.quarantine)?;
    let mut private = ObjectReader::open(&input.private_repo)?;
    let mut tree = parse_commit(&private.read_kind(&input.private_base, ObjectKind::Commit)?)?.tree;
    let mut public_tree = parse_commit(&public.read_kind(&input.public_base, ObjectKind::Commit)?)?.tree;
    let mut parent = input.private_base;
    let mut changed = 0;
    let mut commits = Vec::with_capacity(input.commits.len());
    let mut added = Vec::new();
    let mut seen_blobs = HashSet::new();
    let mut secrets = Vec::new();
    for oid in &input.commits {
        let commit = parse_commit(&public.read_kind(oid, ObjectKind::Commit)?)?;
        check_controls(&commit).map_err(|reason| refuse(reason, Some(oid), None))?;
        secrets.extend(
            scan_text(&String::from_utf8_lossy(&commit.message))
                .into_iter()
                .map(|hit| SecretFinding::Message {
                    public_commit: to_hex(oid),
                    rule: hit.rule,
                    line: hit.line,
                }),
        );
        let mut changes = Vec::new();
        diff(&mut public, "", Some(public_tree), Some(commit.tree), &mut changes)
            .map_err(|stop| super::with_commit(stop, oid))?;
        changed += changes.len();
        if changed > MAX_CHANGED_FILES {
            return Err(refuse(Reason::TooLarge(Cap::ChangedFiles), Some(oid), None));
        }
        for change in &changes {
            gate(&input.filter, change).map_err(|reason| refuse(reason, Some(oid), Some(&change.path)))?;
        }
        let mut directories = BTreeSet::new();
        for change in &changes {
            tree = apply(&mut private, tree, change).map_err(|stop| super::with_commit(stop, oid))?;
            if let Some(new) = &change.new {
                directories.insert(change.path.rsplit_once('/').map_or("", |(dir, _)| dir));
                if new.oid != EMPTY_BLOB && seen_blobs.insert(new.oid) {
                    added.push((new.oid, change.path.clone()));
                }
            }
        }
        for dir in directories {
            if !distinct_without_case(&mut private, tree, dir)? {
                return Err(refuse(Reason::CaseCollision, Some(oid), Some(dir)));
            }
        }
        public_tree = commit.tree;
        let translated = Commit {
            tree,
            parents: vec![parent],
            ..commit
        };
        parent = private.insert(ObjectKind::Commit, encode_commit(&translated));
        commits.push((*oid, parent));
    }

    let blobs: Vec<Oid> = added.iter().map(|(blob, _)| *blob).collect();
    let mut total = 0u64;
    for ((_, path), size) in added.iter().zip(blob_sizes(&input.quarantine, &blobs)?) {
        if size > MAX_BLOB_BYTES {
            return Err(refuse(Reason::TooLarge(Cap::BlobBytes), None, Some(path)));
        }
        total = total.saturating_add(size);
        if total > MAX_PR_BYTES {
            return Err(refuse(Reason::TooLarge(Cap::PrBytes), None, None));
        }
    }
    // Only once every size is known to be under its cap, so that no blob is
    // read whole before that.
    for (blob, path) in &added {
        match scan_blob(&public.read_kind(blob, ObjectKind::Blob)?) {
            Scan::Binary => {}
            Scan::TooLarge => secrets.push(SecretFinding::Unscanned { path: path.clone() }),
            Scan::Hits(hits) => secrets.extend(hits.into_iter().map(|hit| SecretFinding::Blob {
                path: path.clone(),
                rule: hit.rule,
                line: hit.line,
            })),
        }
    }
    Ok(Translated {
        reader: private,
        commits,
        added,
        secrets,
    })
}

/// Bytes git itself never writes into a message or identity other than a
/// newline or a tab: a terminal escape sequence in a message, say (§3.12,
/// "Commit message content"). Messages are otherwise kept verbatim.
fn check_controls(commit: &Commit) -> Result<(), Reason> {
    let control = |bytes: &[u8]| {
        bytes
            .iter()
            .any(|&b| (b < 0x20 && b != b'\n' && b != b'\t') || b == 0x7f)
    };
    if control(&commit.message) || control(&commit.author) || control(&commit.committer) {
        return Err(Reason::ControlChars);
    }
    Ok(())
}

/// The file-level changes from the public tree `old` to `new` under `dir`.
/// A path that turns from a file into a directory, or back, is a deletion
/// and additions, the deletion first so that applying them in order never
/// finds both at once.
fn diff(
    public: &mut ObjectReader,
    dir: &str,
    old: Option<Oid>,
    new: Option<Oid>,
    out: &mut Vec<Change>,
) -> Result<(), Stop> {
    if dir.split('/').count() >= MAX_INBOUND_PATH_DEPTH {
        return Err(refuse(Reason::TooLarge(Cap::TreeDepth), None, Some(dir)));
    }
    let mut names: BTreeMap<Vec<u8>, (Option<TreeEntry>, Option<TreeEntry>)> = BTreeMap::new();
    for entry in read_entries(public, old)? {
        let name = entry.name.clone();
        names.entry(name).or_default().0 = Some(entry);
    }
    for entry in read_entries(public, new)? {
        let name = entry.name.clone();
        names.entry(name).or_default().1 = Some(entry);
    }
    for (name, (old, new)) in names {
        if let (Some(o), Some(n)) = (&old, &new)
            && o.mode == n.mode
            && o.oid == n.oid
        {
            continue;
        }
        // A name that isn't UTF-8 decodes differently per platform, so no
        // check on its text would say what a checkout ends up calling it.
        let Ok(text) = std::str::from_utf8(&name) else {
            let lossy = join(dir, &String::from_utf8_lossy(&name));
            return Err(refuse(Reason::UnsafePath, None, Some(&lossy)));
        };
        let path = join(dir, text);
        if out.len() > MAX_CHANGED_FILES {
            return Err(refuse(Reason::TooLarge(Cap::ChangedFiles), None, None));
        }
        let (old_tree, old_leaf) = split(old.as_ref());
        let (new_tree, new_leaf) = split(new.as_ref());
        match (old_tree, new_tree) {
            (Some(old), Some(new)) => diff(public, &path, Some(old), Some(new), out)?,
            (Some(old), None) => {
                diff(public, &path, Some(old), None, out)?;
                if new_leaf.is_some() {
                    out.push(Change {
                        path,
                        old: None,
                        new: new_leaf,
                    });
                }
            }
            (None, Some(new)) => {
                if old_leaf.is_some() {
                    out.push(Change {
                        path: path.clone(),
                        old: old_leaf,
                        new: None,
                    });
                }
                diff(public, &path, None, Some(new), out)?;
            }
            (None, None) => out.push(Change {
                path,
                old: old_leaf,
                new: new_leaf,
            }),
        }
    }
    Ok(())
}

fn split(entry: Option<&TreeEntry>) -> (Option<Oid>, Option<Leaf>) {
    match entry {
        Some(entry) if entry.is_tree() => (Some(entry.oid), None),
        Some(entry) => (None, Some(Leaf::of(entry))),
        None => (None, None),
    }
}

fn read_entries(reader: &mut ObjectReader, tree: Option<Oid>) -> anyhow::Result<Vec<TreeEntry>> {
    match tree {
        Some(tree) => parse_tree(&reader.read_kind(&tree, ObjectKind::Tree)?),
        None => Ok(Vec::new()),
    }
}

/// The checks on one changed path that need neither side's content. The
/// kind is checked before the filter, so a symlink or a submodule is named
/// as such whatever the filter would say about its path.
fn gate(filter: &Filter, change: &Change) -> Result<(), Reason> {
    validate_inbound_path(&change.path).map_err(|_| Reason::UnsafePath)?;
    for leaf in change.old.iter().chain(&change.new) {
        match leaf.mode.as_slice() {
            MODE_FILE | MODE_EXECUTABLE => {}
            MODE_SYMLINK => return Err(Reason::Symlink),
            MODE_GITLINK => return Err(Reason::Gitlink),
            _ => return Err(Reason::UnsupportedMode),
        }
    }
    if filter.excludes_published_path(&change.path, EntryKind::File).is_some() {
        return Err(Reason::ExcludedPath);
    }
    if sensitive(&change.path) {
        return Err(Reason::SensitivePath);
    }
    Ok(())
}

fn sensitive(path: &str) -> bool {
    let top = path.split('/').next().unwrap_or(path);
    let base = path.rsplit('/').next().unwrap_or(path);
    SENSITIVE_NAMES.iter().any(|name| base.eq_ignore_ascii_case(name))
        || SENSITIVE_TOP_LEVEL.iter().any(|name| top.eq_ignore_ascii_case(name))
        || top.to_ascii_lowercase().starts_with(".woodpecker")
}

/// `root` with `change` applied, every tree along its path rebuilt and
/// every sibling kept. Refuses unless the private tree holds, at the path,
/// exactly what the public side held before the change: the same mode and
/// blob, or nothing (and nothing in the way of the directories it needs).
fn apply(private: &mut ObjectReader, root: Oid, change: &Change) -> Result<Oid, Stop> {
    let segments: Vec<&str> = change.path.split('/').collect();
    let entries = read_entries(private, Some(root))?;
    let entries = apply_in(private, entries, &segments, change)?;
    Ok(private.insert(ObjectKind::Tree, encode_tree(&entries)))
}

fn apply_in(
    private: &mut ObjectReader,
    mut entries: Vec<TreeEntry>,
    segments: &[&str],
    change: &Change,
) -> Result<Vec<TreeEntry>, Stop> {
    let precondition = || refuse(Reason::PreconditionFailed, None, Some(&change.path));
    let Some((name, rest)) = segments.split_first() else {
        return Err(anyhow::anyhow!("an empty path reached the overlay").into());
    };
    let current = entries
        .iter()
        .position(|e| e.name == name.as_bytes())
        .map(|at| entries.remove(at));
    let replacement = if rest.is_empty() {
        if current.as_ref().map(Leaf::of) != change.old {
            return Err(precondition());
        }
        change.new.as_ref().map(|leaf| TreeEntry {
            mode: leaf.mode.clone(),
            name: name.as_bytes().to_vec(),
            oid: leaf.oid,
        })
    } else {
        let below = match &current {
            Some(entry) if entry.is_tree() => read_entries(private, Some(entry.oid))?,
            Some(_) => return Err(precondition()),
            None if change.old.is_some() => return Err(precondition()),
            None => Vec::new(),
        };
        let below = apply_in(private, below, rest, change)?;
        (!below.is_empty()).then(|| TreeEntry {
            mode: MODE_TREE.to_vec(),
            name: name.as_bytes().to_vec(),
            oid: private.insert(ObjectKind::Tree, encode_tree(&below)),
        })
    };
    if let Some(entry) = replacement {
        let at = entries.partition_point(|e| git_order(e, &entry) == Ordering::Less);
        entries.insert(at, entry);
    }
    Ok(entries)
}

/// git's tree order: by name, a directory compared as if its name ended in
/// `/` (`base_name_compare` in git's `tree.c`). A tree in any other order
/// is one git itself never writes, so its id would match no real commit.
fn git_order(a: &TreeEntry, b: &TreeEntry) -> Ordering {
    let key = |e: &TreeEntry| {
        let mut key = e.name.clone();
        if e.is_tree() {
            key.push(b'/');
        }
        key
    };
    key(a).cmp(&key(b))
}

/// The key two names must not share in one directory: what a case-insensitive,
/// normalising filesystem (macOS, Windows) would treat as the same name.
///
/// A valid UTF-8 name goes through `to_uppercase` then `to_lowercase` (so `ß`
/// meets `SS`) and then NFKC. No full Unicode case folding is available
/// without a new dependency (`icu_casemap`), so the limit is that of Rust's
/// own case mapping: locale-specific folds (Turkish dotless `ı`) and
/// foldings outside the simple and special-casing tables are not seen. NFKC
/// is wider than canonical equivalence (it also joins compatibility forms
/// such as full-width letters), which only errs towards refusing. A name
/// that is not UTF-8 is compared by its ASCII lowercase bytes.
///
/// Normalisation per [`icu_normalizer::ComposingNormalizerBorrowed::new_nfkc`](https://docs.rs/icu_normalizer/2.3.0/icu_normalizer/struct.ComposingNormalizerBorrowed.html).
fn collision_key(name: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(name) {
        Ok(name) => ComposingNormalizerBorrowed::new_nfkc()
            .normalize(&name.to_uppercase().to_lowercase())
            .into_owned()
            .into_bytes(),
        // `Utf8Error` has no variant worth telling apart: any non-UTF-8 name
        // gets the same byte comparison, since Unicode folding cannot apply.
        Err(_) => name.to_ascii_lowercase(),
    }
}

/// Whether the directory `dir` of `root` has no two names that collide
/// under [`collision_key`]. A directory no longer there has none.
fn distinct_without_case(private: &mut ObjectReader, root: Oid, dir: &str) -> anyhow::Result<bool> {
    let mut tree = root;
    for segment in dir.split('/').filter(|s| !s.is_empty()) {
        let entries = read_entries(private, Some(tree))?;
        match entries.iter().find(|e| e.is_tree() && e.name == segment.as_bytes()) {
            Some(entry) => tree = entry.oid,
            None => return Ok(true),
        }
    }
    let mut seen = HashSet::new();
    Ok(read_entries(private, Some(tree))?
        .iter()
        .all(|e| seen.insert(collision_key(&e.name))))
}

pub(super) struct RoundTrip {
    pub reader: ObjectReader,
    pub filter: Filter,
    pub identities: Vec<IdentityMapping>,
    pub private_base: Oid,
    /// The public base and its tree, as the forward mapping records them.
    pub public_base: (Oid, Oid),
    pub commits: Vec<(Oid, Oid)>,
}

/// The gate of §3.3 step 3: the forward rewrite, seeded with the private
/// base, has to publish each translated commit as the very public commit
/// it came from, byte for byte. Anything else would make the next publish
/// a history parallel to the pull request's own commits.
pub(super) fn round_trip(mut input: RoundTrip) -> Result<(), Stop> {
    let mut rewriter = Rewriter::new(input.filter, input.identities);
    rewriter.seed(input.private_base, Some(input.public_base));
    for (public, private) in &input.commits {
        rewriter.rewrite_commit(&mut input.reader, *private)?;
        if rewriter.mapped(private) != Some(Some(*public)) {
            return Err(refuse(Reason::RoundTripMismatch, Some(public), None));
        }
    }
    Ok(())
}
