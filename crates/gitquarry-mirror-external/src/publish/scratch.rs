//! The local repository a publish run writes its public objects into before
//! pushing them: a fresh `git init`, never a clone of the private repo and
//! never with alternates, so an object is only ever in it because this
//! crate wrote it there or because an earlier run of the same mirror did
//! (see [`super::history`]).

use super::objects::{ObjectKind, Oid, from_hex, to_hex};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub(super) fn init_scratch(repo: &Path) -> Result<()> {
    // SHA-1 named explicitly: the ids here are computed as SHA-1, and a
    // global `init.defaultObjectFormat` must not change what gets written.
    let out = Command::new("git")
        .args(["init", "--bare", "--quiet", "--object-format=sha1", "--end-of-options"])
        .arg(repo)
        .output()
        .context("couldn't run git init")?;
    if !out.status.success() {
        bail!("git init failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Writes `objects` into `repo` through `git hash-object`, so git computes
/// each id itself, and fails unless every id is the one computed here: the
/// gates and the preview speak about the computed ids, and a mismatch would
/// mean what gets pushed is not what was checked.
pub(super) fn write_objects(
    repo: &Path,
    stage: &Path,
    kind: ObjectKind,
    objects: impl IntoIterator<Item = (Oid, Vec<u8>)>,
) -> Result<Vec<Oid>> {
    let mut expected = Vec::new();
    let mut paths = Vec::new();
    for (i, (oid, body)) in objects.into_iter().enumerate() {
        let path = stage.join(format!("{i}"));
        std::fs::write(&path, &body).with_context(|| format!("staging object {}", to_hex(&oid)))?;
        expected.push(oid);
        paths.push(path);
    }
    if expected.is_empty() {
        return Ok(expected);
    }
    let type_name = match kind {
        ObjectKind::Blob => "blob",
        ObjectKind::Tree => "tree",
        ObjectKind::Commit => "commit",
        ObjectKind::Tag => "tag",
    };
    let mut child = Command::new("git")
        .env("GIT_DIR", repo)
        .args(["hash-object", "-w", "--no-filters", "--stdin-paths", "-t", type_name])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run git hash-object")?;
    // A writer thread, or a large batch fills the stdout pipe while this
    // side is still blocked writing stdin.
    let mut stdin = child.stdin.take().context("git hash-object started without stdin")?;
    let list: Vec<u8> = paths
        .iter()
        .flat_map(|p| {
            let mut line = p.to_string_lossy().into_owned().into_bytes();
            line.push(b'\n');
            line
        })
        .collect();
    let feeder = std::thread::spawn(move || stdin.write_all(&list));
    let out = child.wait_with_output().context("waiting for git hash-object")?;
    feeder
        .join()
        .map_err(|_| anyhow::anyhow!("the git hash-object feeder panicked"))?
        .context("writing to git hash-object")?;
    for path in &paths {
        let _ = std::fs::remove_file(path);
    }
    if !out.status.success() {
        bail!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let written: Vec<&str> = text.lines().collect();
    if written.len() != expected.len() {
        bail!(
            "git hash-object wrote {} objects, {} were expected",
            written.len(),
            expected.len()
        );
    }
    for (computed, written) in expected.iter().zip(written) {
        if to_hex(computed) != written {
            bail!(
                "{type_name} computed as {} was written as {written}; refusing to publish what wasn't checked",
                to_hex(computed)
            );
        }
    }
    Ok(expected)
}

/// Every object in `repo`, whatever put it there.
pub(super) fn written_by(repo: &Path) -> Result<HashSet<Oid>> {
    let out = Command::new("git")
        .env("GIT_DIR", repo)
        .args(["cat-file", "--batch-all-objects", "--batch-check=%(objectname)"])
        .output()
        .context("couldn't run git cat-file")?;
    if !out.status.success() {
        bail!("git cat-file failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    String::from_utf8_lossy(&out.stdout).lines().map(from_hex).collect()
}

/// Every loose object in `repo`: what this run wrote, as opposed to what
/// came in through a downloaded pack.
pub(super) fn loose_objects(repo: &Path) -> Result<HashSet<Oid>> {
    let mut found = HashSet::new();
    let objects = repo.join("objects");
    for dir in std::fs::read_dir(&objects).with_context(|| format!("listing {}", objects.display()))? {
        let dir = dir.with_context(|| format!("listing {}", objects.display()))?;
        let prefix = dir.file_name().to_string_lossy().into_owned();
        if prefix.len() != 2 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        for file in std::fs::read_dir(dir.path()).with_context(|| format!("listing {}", dir.path().display()))? {
            let file = file.with_context(|| format!("listing {}", dir.path().display()))?;
            let rest = file.file_name().to_string_lossy().into_owned();
            if let Ok(oid) = from_hex(&format!("{prefix}{rest}")) {
                found.insert(oid);
            }
        }
    }
    Ok(found)
}

/// One pack holding exactly `objects`, written under `out_dir`, as
/// `(base name, .pack, .idx)`; `None` when there is nothing to pack. No
/// `--revs`: the pack holds the listed objects and nothing reachable from
/// them, so it never carries an object this run didn't decide to publish.
pub(super) fn pack_objects(repo: &Path, objects: &[Oid], out_dir: &Path) -> Result<Option<(String, PathBuf, PathBuf)>> {
    if objects.is_empty() {
        return Ok(None);
    }
    let mut child = Command::new("git")
        .env("GIT_DIR", repo)
        .args(["pack-objects", "-q"])
        .arg(out_dir.join("pack"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run git pack-objects")?;
    let mut stdin = child.stdin.take().context("git pack-objects started without stdin")?;
    let list = objects
        .iter()
        .fold(String::with_capacity(objects.len() * 41), |mut list, oid| {
            list.push_str(&to_hex(oid));
            list.push('\n');
            list
        });
    let feeder = std::thread::spawn(move || stdin.write_all(list.as_bytes()));
    let out = child.wait_with_output().context("waiting for git pack-objects")?;
    feeder
        .join()
        .map_err(|_| anyhow::anyhow!("the git pack-objects feeder panicked"))?
        .context("writing to git pack-objects")?;
    if !out.status.success() {
        bail!(
            "git pack-objects failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("git pack-objects named its pack {hash:?}");
    }
    let base = format!("pack-{hash}");
    Ok(Some((
        base,
        out_dir.join(format!("pack-{hash}.pack")),
        out_dir.join(format!("pack-{hash}.idx")),
    )))
}

/// A `git cat-file --batch-check` process answering "is this object here?"
/// for a run's own repository.
pub(super) struct Presence {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Presence {
    pub(super) fn open(repo: &Path) -> Result<Self> {
        let mut child = Command::new("git")
            .env("GIT_DIR", repo)
            .args(["cat-file", "--batch-check=%(objectname)"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("couldn't run git cat-file --batch-check")?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            bail!("git cat-file --batch-check started without its pipes");
        };
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    pub(super) fn contains(&mut self, oid: &Oid) -> Result<bool> {
        let hex = to_hex(oid);
        writeln!(self.stdin, "{hex}").context("writing to git cat-file")?;
        self.stdin.flush().context("writing to git cat-file")?;
        let mut line = String::new();
        self.stdout.read_line(&mut line).context("reading from git cat-file")?;
        Ok(line.trim() == hex)
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
