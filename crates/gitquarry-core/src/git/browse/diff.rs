//! Single-commit detail, diffstats, ranged diffs, and mailable patches.

use super::log::{CommitInfo, log};
use crate::git::run;
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::path::Path;

/// Detail of a commit: metadata + full diff (`git show`).
pub async fn show_commit(git_dir: &Path, sha: &str) -> Result<(CommitInfo, String)> {
    let info = log(git_dir, sha, 1, None)
        .await?
        .into_iter()
        .next()
        .with_context(|| format!("commit {sha} not found"))?;

    let out = run(
        Some(git_dir),
        &[
            OsStr::new("show"),
            OsStr::new("--format="),
            OsStr::new("--end-of-options"),
            OsStr::new(sha),
        ],
    )
    .await?;
    Ok((info, String::from_utf8_lossy(&out).into_owned()))
}

/// Diffstat summary of a commit (`git show --stat`), without the patch body.
pub async fn diffstat(git_dir: &Path, sha: &str) -> Result<String> {
    let out = run(
        Some(git_dir),
        &[
            OsStr::new("show"),
            OsStr::new("--stat"),
            OsStr::new("--format="),
            OsStr::new("--end-of-options"),
            OsStr::new(sha),
        ],
    )
    .await?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Full diff between two revs. Three-dot (`base...head`): changes on `head`
/// since it diverged from `base`, not polluted by unrelated commits `base`
/// gained meanwhile — the semantics a "compare" view wants, not a plain
/// two-dot diff.
pub async fn diff_range(git_dir: &Path, base: &str, head: &str) -> Result<String> {
    let range = format!("{base}...{head}");
    let out = run(
        Some(git_dir),
        &[OsStr::new("diff"), OsStr::new("--end-of-options"), OsStr::new(&range)],
    )
    .await?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Diffstat summary of [`diff_range`], without the patch body.
pub async fn diffstat_range(git_dir: &Path, base: &str, head: &str) -> Result<String> {
    let range = format!("{base}...{head}");
    let out = run(
        Some(git_dir),
        &[
            OsStr::new("diff"),
            OsStr::new("--stat"),
            OsStr::new("--end-of-options"),
            OsStr::new(&range),
        ],
    )
    .await?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Every path touched by [`diff_range`]'s same three-dot range, one per
/// line, no patch body — what `git::codeowners` matches against, and what a
/// required-reviewers/CODEOWNERS merge gate needs instead of the raw diff
/// text every other caller in this file wants.
pub async fn diff_range_name_only(git_dir: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    let range = format!("{base}...{head}");
    let out = run(
        Some(git_dir),
        &[
            OsStr::new("diff"),
            OsStr::new("--name-only"),
            OsStr::new("--end-of-options"),
            OsStr::new(&range),
        ],
    )
    .await?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// `git format-patch --stdout -1 <sha>`: the commit as a mailable patch.
pub async fn format_patch(git_dir: &Path, sha: &str) -> Result<String> {
    let out = run(
        Some(git_dir),
        &[
            OsStr::new("format-patch"),
            OsStr::new("--stdout"),
            OsStr::new("-1"),
            OsStr::new("--end-of-options"),
            OsStr::new(sha),
        ],
    )
    .await?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}
