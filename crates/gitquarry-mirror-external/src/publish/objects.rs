//! Read-only access to the private repo's objects, and the git object
//! encoding the rewrite needs to compute public ids without writing them.
//!
//! Synchronous on purpose: one preview reads thousands of objects through a
//! single `git cat-file --batch` pipe, and [`super::preview`] runs the whole
//! computation on a blocking thread rather than awaiting each round trip.

use anyhow::{Context, Result, bail};
use sha1::{Digest, Sha1};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub(crate) type Oid = [u8; 20];

/// Git's mode for a gitlink (submodule) entry, as stored in a tree.
const MODE_GITLINK: &[u8] = b"160000";
const MODE_TREE: &[u8] = b"40000";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    Blob,
    Tree,
    Commit,
    Tag,
}

/// A `git cat-file --batch` process over the private repo. Only ever asked
/// for objects the caller already decided to read, which is how an excluded
/// blob is never loaded: nothing here walks on its own.
pub(crate) struct ObjectReader {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ObjectReader {
    pub(crate) fn open(git_dir: &Path) -> Result<Self> {
        check_sha1_repo(git_dir)?;
        let mut child = Command::new("git")
            .env("GIT_DIR", git_dir)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("couldn't run git cat-file --batch")?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            bail!("git cat-file --batch started without its pipes");
        };
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    pub(crate) fn read(&mut self, oid: &Oid) -> Result<(ObjectKind, Vec<u8>)> {
        let hex = to_hex(oid);
        writeln!(self.stdin, "{hex}").context("writing to git cat-file")?;
        self.stdin.flush().context("writing to git cat-file")?;
        let mut header = String::new();
        self.stdout
            .read_line(&mut header)
            .context("reading from git cat-file")?;
        let mut parts = header.split_whitespace();
        let (Some(_), Some(kind), Some(size)) = (parts.next(), parts.next(), parts.next()) else {
            bail!("object {hex} is missing from the repository");
        };
        let kind = match kind {
            "blob" => ObjectKind::Blob,
            "tree" => ObjectKind::Tree,
            "commit" => ObjectKind::Commit,
            "tag" => ObjectKind::Tag,
            other => bail!("object {hex} has unknown type {other}"),
        };
        let size: usize = size.parse().with_context(|| format!("bad size for object {hex}"))?;
        let mut body = vec![0; size + 1];
        self.stdout
            .read_exact(&mut body)
            .with_context(|| format!("reading object {hex}"))?;
        body.truncate(size);
        Ok((kind, body))
    }

    pub(crate) fn read_kind(&mut self, oid: &Oid, want: ObjectKind) -> Result<Vec<u8>> {
        let (kind, body) = self.read(oid)?;
        if kind != want {
            bail!("object {} is a {kind:?}, expected a {want:?}", to_hex(oid));
        }
        Ok(body)
    }
}

impl Drop for ObjectReader {
    fn drop(&mut self) {
        // Otherwise an aborted preview leaves the process reading a pipe
        // nobody will ever write to again.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The rewrite computes SHA-1 ids itself, so a SHA-256 repository would get
/// ids that name nothing. Refused up front instead.
fn check_sha1_repo(git_dir: &Path) -> Result<()> {
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["config", "--get", "extensions.objectformat"])
        .output()
        .context("couldn't run git config")?;
    let format = String::from_utf8_lossy(&out.stdout).trim().to_ascii_lowercase();
    if !format.is_empty() && format != "sha1" {
        bail!("publish mirrors support only SHA-1 repositories, this one is {format}");
    }
    Ok(())
}

/// One entry of a tree object, its bytes kept as stored so that re-encoding
/// a filtered tree reproduces git's own encoding exactly.
#[derive(Debug, Clone)]
pub(crate) struct TreeEntry {
    pub mode: Vec<u8>,
    pub name: Vec<u8>,
    pub oid: Oid,
}

impl TreeEntry {
    pub(crate) fn is_tree(&self) -> bool {
        self.mode == MODE_TREE
    }

    pub(crate) fn is_gitlink(&self) -> bool {
        self.mode == MODE_GITLINK
    }

    pub(crate) fn name_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.name)
    }
}

pub(crate) fn parse_tree(body: &[u8]) -> Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let space = rest.iter().position(|&b| b == b' ').context("malformed tree entry")?;
        let nul = rest.iter().position(|&b| b == 0).context("malformed tree entry")?;
        if nul < space || rest.len() < nul + 21 {
            bail!("malformed tree entry");
        }
        let mut oid = [0; 20];
        oid.copy_from_slice(&rest[nul + 1..nul + 21]);
        entries.push(TreeEntry {
            mode: rest[..space].to_vec(),
            name: rest[space + 1..nul].to_vec(),
            oid,
        });
        rest = &rest[nul + 21..];
    }
    Ok(entries)
}

/// Re-encodes entries that came out of [`parse_tree`], in the order given.
/// A subset of a valid tree, in its original order, is still sorted the way
/// git requires, so no re-sorting happens here.
pub(crate) fn encode_tree(entries: &[TreeEntry]) -> Vec<u8> {
    let mut body = Vec::new();
    for entry in entries {
        body.extend_from_slice(&entry.mode);
        body.push(b' ');
        body.extend_from_slice(&entry.name);
        body.push(0);
        body.extend_from_slice(&entry.oid);
    }
    body
}

/// The id git would give `body` stored as an object of `kind`.
pub(crate) fn hash_object(kind: ObjectKind, body: &[u8]) -> Oid {
    let kind = match kind {
        ObjectKind::Blob => "blob",
        ObjectKind::Tree => "tree",
        ObjectKind::Commit => "commit",
        ObjectKind::Tag => "tag",
    };
    let mut hasher = Sha1::new();
    hasher.update(format!("{kind} {}\0", body.len()).as_bytes());
    hasher.update(body);
    hasher.finalize().into()
}

/// The empty blob: identical in every repository, carries no content, so it
/// is never evidence of a leak.
pub(crate) const EMPTY_BLOB: Oid = [
    0xe6, 0x9d, 0xe2, 0x9b, 0xb2, 0xd1, 0xd6, 0x43, 0x4b, 0x8b, 0x29, 0xae, 0x77, 0x5a, 0xd8, 0xc2, 0xe4, 0x8c, 0x53,
    0x91,
];

pub(crate) fn to_hex(oid: &Oid) -> String {
    use std::fmt::Write as _;
    oid.iter().fold(String::with_capacity(40), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

pub(crate) fn from_hex(hex: &str) -> Result<Oid> {
    let bytes = hex.as_bytes();
    if bytes.len() != 40 {
        bail!("invalid object id {hex:?}");
    }
    let mut oid = [0; 20];
    for (i, pair) in bytes.chunks(2).enumerate() {
        let pair = std::str::from_utf8(pair).with_context(|| format!("invalid object id {hex:?}"))?;
        oid[i] = u8::from_str_radix(pair, 16).with_context(|| format!("invalid object id {hex:?}"))?;
    }
    Ok(oid)
}

/// A commit split into the parts the rewrite touches; every other header
/// line is kept verbatim, in order.
#[derive(Debug)]
pub(crate) struct Commit {
    pub tree: Oid,
    pub parents: Vec<Oid>,
    pub author: Vec<u8>,
    pub committer: Vec<u8>,
    /// Header lines other than the above, each with its continuation lines,
    /// minus the signature headers a rewritten commit can't carry.
    pub other_headers: Vec<Vec<u8>>,
    pub message: Vec<u8>,
}

/// Headers whose content signs the *private* commit, which would be a
/// signature that no longer verifies on the public one.
const DROPPED_HEADERS: [&[u8]; 3] = [b"gpgsig", b"gpgsig-sha256", b"mergetag"];

pub(crate) fn parse_commit(body: &[u8]) -> Result<Commit> {
    let split = body.windows(2).position(|w| w == b"\n\n").map_or(body.len(), |i| i + 1);
    let (head, message) = body.split_at(split);
    let message = message.strip_prefix(b"\n").unwrap_or(message).to_vec();
    let mut tree = None;
    let mut parents = Vec::new();
    let mut author = None;
    let mut committer = None;
    let mut other_headers: Vec<Vec<u8>> = Vec::new();
    let mut dropping = false;
    for line in head.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        if line.starts_with(b" ") {
            if !dropping && let Some(last) = other_headers.last_mut() {
                last.push(b'\n');
                last.extend_from_slice(line);
            }
            continue;
        }
        dropping = false;
        let (key, value) = match line.iter().position(|&b| b == b' ') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, &line[line.len()..]),
        };
        match key {
            b"tree" => tree = Some(from_hex(&String::from_utf8_lossy(value))?),
            b"parent" => parents.push(from_hex(&String::from_utf8_lossy(value))?),
            b"author" => author = Some(value.to_vec()),
            b"committer" => committer = Some(value.to_vec()),
            k if DROPPED_HEADERS.contains(&k) => dropping = true,
            _ => other_headers.push(line.to_vec()),
        }
    }
    Ok(Commit {
        tree: tree.context("commit without a tree")?,
        parents,
        author: author.context("commit without an author")?,
        committer: committer.context("commit without a committer")?,
        other_headers,
        message,
    })
}

pub(crate) fn encode_commit(commit: &Commit) -> Vec<u8> {
    let mut body = Vec::with_capacity(256 + commit.message.len());
    body.extend_from_slice(b"tree ");
    body.extend_from_slice(to_hex(&commit.tree).as_bytes());
    body.push(b'\n');
    for parent in &commit.parents {
        body.extend_from_slice(b"parent ");
        body.extend_from_slice(to_hex(parent).as_bytes());
        body.push(b'\n');
    }
    body.extend_from_slice(b"author ");
    body.extend_from_slice(&commit.author);
    body.push(b'\n');
    body.extend_from_slice(b"committer ");
    body.extend_from_slice(&commit.committer);
    body.push(b'\n');
    for header in &commit.other_headers {
        body.extend_from_slice(header);
        body.push(b'\n');
    }
    body.push(b'\n');
    body.extend_from_slice(&commit.message);
    body
}

/// The `Name <email>` / `timestamp tz` halves of an author or committer
/// value. Split on the *last* `<`…`>` pair, the one git itself writes.
pub(crate) struct Ident<'a> {
    pub name: &'a [u8],
    pub email: &'a [u8],
    pub when: &'a [u8],
}

pub(crate) fn split_ident(value: &[u8]) -> Option<Ident<'_>> {
    let close = value.iter().rposition(|&b| b == b'>')?;
    let open = value[..close].iter().rposition(|&b| b == b'<')?;
    let name = value[..open].strip_suffix(b" ").unwrap_or(&value[..open]);
    let when = value[close + 1..].strip_prefix(b" ").unwrap_or(&value[close + 1..]);
    Some(Ident {
        name,
        email: &value[open + 1..close],
        when,
    })
}

/// Commits reachable from `tip`, parents before children.
pub(crate) fn topo_order(git_dir: &Path, tip: &Oid) -> Result<Vec<Oid>> {
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["rev-list", "--topo-order", "--reverse", "--end-of-options"])
        .arg(to_hex(tip))
        .output()
        .context("couldn't run git rev-list")?;
    if !out.status.success() {
        bail!("git rev-list failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    String::from_utf8_lossy(&out.stdout).lines().map(from_hex).collect()
}

/// The newest `limit` commits reachable from `tip`, parents before children,
/// and whether older ones were left out. The oldest commit of a truncated
/// window has parents outside it.
pub(crate) fn topo_order_newest(git_dir: &Path, tip: &Oid, limit: usize) -> Result<(Vec<Oid>, bool)> {
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["rev-list", "--topo-order", "--reverse"])
        // One past the limit, only to learn whether there is more.
        .arg(format!("--max-count={}", limit.saturating_add(1)))
        .args(["--end-of-options"])
        .arg(to_hex(tip))
        .output()
        .context("couldn't run git rev-list")?;
    if !out.status.success() {
        bail!("git rev-list failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut commits = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(from_hex)
        .collect::<Result<Vec<_>>>()?;
    let truncated = commits.len() > limit;
    if truncated {
        commits.remove(0);
    }
    Ok((commits, truncated))
}

/// Every ref under `refs/heads/` and `refs/tags/`, with the commit it
/// finally points at (an annotated tag peeled) — `None` for a ref that
/// doesn't end at a commit.
pub(crate) fn list_branches_and_tags(git_dir: &Path) -> Result<Vec<(String, Option<Oid>)>> {
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args([
            "for-each-ref",
            "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)",
            "refs/heads/",
            "refs/tags/",
        ])
        .output()
        .context("couldn't run git for-each-ref")?;
    if !out.status.success() {
        bail!(
            "git for-each-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut refs = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [name, kind, oid, peeled_kind, peeled_oid] = fields.as_slice() else {
            continue;
        };
        let commit = match (*kind, *peeled_kind) {
            ("commit", _) => Some(from_hex(oid)?),
            ("tag", "commit") => Some(from_hex(peeled_oid)?),
            _ => None,
        };
        refs.push((name.to_string(), commit));
    }
    Ok(refs)
}
