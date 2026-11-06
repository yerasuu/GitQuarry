//! The quarantine repo: a fresh bare repo holding only what the public
//! remote sent for one pull request, read through plumbing only (no
//! checkout, so nothing in the pull request's trees is ever a file here).

use super::MAX_PR_COMMITS;
use crate::git_remote::isolate;
use crate::publish::objects::{Oid, from_hex, to_hex};
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Where the fetch puts the pull request's head and the public base
/// branch's tip. Under `refs/inbound/` so that neither can be taken for a
/// branch of the quarantine repo itself.
pub(super) const HEAD_REF: &str = "refs/inbound/head";
pub(super) const BASE_REF: &str = "refs/inbound/base";

/// The pull request as the quarantine repo has it.
pub(super) struct PublicRange {
    pub head: Oid,
    /// Merge bases of the head and the public branch: one in the normal
    /// case, none when they share no history.
    pub bases: Vec<Oid>,
    /// The commits on top of the single merge base, parents first, each
    /// with its parents, at most [`MAX_PR_COMMITS`] of them.
    pub commits: Vec<(Oid, Vec<Oid>)>,
    /// There are more than [`MAX_PR_COMMITS`].
    pub truncated: bool,
    /// With no merge base only: the head's first-parent ancestry, newest
    /// first and as long as the commit cap allows, to tell an old epoch's
    /// base from a foreign one.
    pub ancestry: Vec<Oid>,
}

pub(super) fn public_range(quarantine: &Path) -> Result<PublicRange> {
    let head = rev_parse_commit(quarantine, HEAD_REF)?;
    let tip = rev_parse_commit(quarantine, BASE_REF)?;
    let bases = merge_bases(quarantine, &head, &tip)?;
    let limit = format!("--max-count={}", MAX_PR_COMMITS + 1);
    let (mut commits, mut ancestry) = (Vec::new(), Vec::new());
    if let [base] = bases.as_slice() {
        // Newest first under --max-count, then reversed by git itself, so
        // the oldest commit is dropped when there is one too many.
        let base = format!("^{}", to_hex(base));
        let out = git(
            quarantine,
            &[
                "rev-list",
                "--topo-order",
                "--reverse",
                "--parents",
                &limit,
                "--end-of-options",
                &to_hex(&head),
                &base,
            ],
        )?;
        for line in out.lines() {
            let mut ids = line.split(' ').map(from_hex);
            let commit = ids.next().context("empty line from git rev-list")??;
            commits.push((commit, ids.collect::<Result<Vec<_>>>()?));
        }
    } else if bases.is_empty() {
        let out = git(
            quarantine,
            &["rev-list", "--first-parent", &limit, "--end-of-options", &to_hex(&head)],
        )?;
        ancestry = out.lines().map(from_hex).collect::<Result<_>>()?;
    }
    let truncated = commits.len() > MAX_PR_COMMITS;
    if truncated {
        commits.remove(0);
    }
    Ok(PublicRange {
        head,
        bases,
        commits,
        truncated,
        ancestry,
    })
}

/// The type and size of each of `blobs`, from `cat-file --batch-check`,
/// which reads object headers and never content.
pub(super) fn blob_sizes(quarantine: &Path, blobs: &[Oid]) -> Result<Vec<u64>> {
    if blobs.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git_command(quarantine)?
        .args(["cat-file", "--batch-check=%(objectname) %(objecttype) %(objectsize)"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run git cat-file --batch-check")?;
    let mut stdin = child.stdin.take().context("git cat-file started without stdin")?;
    let list = blobs
        .iter()
        .fold(String::with_capacity(blobs.len() * 41), |mut list, oid| {
            list.push_str(&to_hex(oid));
            list.push('\n');
            list
        });
    // A writer thread, or a long list fills the stdout pipe while this side
    // is still blocked writing stdin.
    let feeder = std::thread::spawn(move || stdin.write_all(list.as_bytes()));
    let out = child.wait_with_output().context("waiting for git cat-file")?;
    feeder
        .join()
        .map_err(|_| anyhow::anyhow!("the git cat-file feeder panicked"))?
        .context("writing to git cat-file")?;
    if !out.status.success() {
        bail!("git cat-file failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut sizes = Vec::with_capacity(blobs.len());
    for (oid, line) in blobs.iter().zip(text.lines()) {
        let hex = to_hex(oid);
        match line.split(' ').collect::<Vec<_>>().as_slice() {
            [name, "blob", size] if *name == hex => {
                sizes.push(size.parse().with_context(|| format!("bad size for blob {hex}"))?);
            }
            _ => bail!("{hex} is not a blob of the pull request: {line}"),
        }
    }
    if sizes.len() != blobs.len() {
        bail!("git cat-file answered for {} of {} blobs", sizes.len(), blobs.len());
    }
    Ok(sizes)
}

fn rev_parse_commit(quarantine: &Path, name: &str) -> Result<Oid> {
    let spec = format!("{name}^{{commit}}");
    from_hex(git(quarantine, &["rev-parse", "--verify", "--quiet", &spec])?.trim())
        .with_context(|| format!("the fetch left no commit at {name}"))
}

fn merge_bases(quarantine: &Path, head: &Oid, tip: &Oid) -> Result<Vec<Oid>> {
    let out = git_command(quarantine)?
        .args(["merge-base", "--all", "--end-of-options"])
        .args([to_hex(head), to_hex(tip)])
        .output()
        .context("couldn't run git merge-base")?;
    // Exit status 1 with nothing printed is git's answer for "no common
    // ancestor", not a failure.
    if !out.status.success() && !(out.status.code() == Some(1) && out.stdout.is_empty()) {
        bail!("git merge-base failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    String::from_utf8_lossy(&out.stdout).lines().map(from_hex).collect()
}

fn git(quarantine: &Path, args: &[&str]) -> Result<String> {
    let out = git_command(quarantine)?
        .args(args)
        .output()
        .with_context(|| format!("couldn't run git {}", args.first().unwrap_or(&"")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_command(quarantine: &Path) -> Result<Command> {
    let mut cmd = Command::new("git");
    isolate(&mut cmd)?;
    cmd.env("GIT_DIR", quarantine);
    Ok(cmd)
}
