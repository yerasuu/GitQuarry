//! Read-only git operations for the web UI: tree/blob/log/blame/refs/stats/
//! archive. Kept separate from [`super`] because none of this is needed by
//! the git:// / ssh:// transports or the pre-receive hook — it only exists
//! to render pages. Split by concern (one file per browsing feature) rather
//! than kept as a single module: each submodule re-exports its public items
//! here so every existing `browse::xxx` call site keeps working unchanged.

mod archive;
mod blame;
mod blob;
mod diff;
mod last_touched;
mod log;
mod refs;
mod signature;
mod submodules;
mod tree;

pub use archive::*;
pub use blame::*;
pub use blob::*;
pub use diff::*;
pub use last_touched::*;
pub use log::*;
pub use refs::*;
pub use signature::*;
pub use submodules::*;
pub use tree::*;

use anyhow::{Context, Result};
use std::path::Path;
use tokio::process::Command;

/// Symbolic ref HEAD points to (`refs/heads/main`), or None if the repo is
/// empty / HEAD doesn't point to anything resolvable.
pub async fn resolve_head(git_dir: &Path) -> Result<Option<String>> {
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .await
        .context("couldn't run git symbolic-ref -q HEAD")?;
    if !out.status.success() {
        return Ok(None);
    }
    let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if head.is_empty() {
        return Ok(None);
    }
    // `symbolic-ref` alone isn't enough: it succeeds even for an unborn
    // branch (HEAD points at `refs/heads/master`, but that ref has no
    // commit yet) — exactly what a bare repo `git init --bare` just
    // created looks like, e.g. right after `/_new`, before the first push.
    // Without this check that unborn ref would be handed to `ls-tree`/`log`
    // downstream as if it named a real commit and fail loudly.
    let verify = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["rev-parse", "--verify", "-q", "--end-of-options", &head])
        .stdout(std::process::Stdio::null())
        .status()
        .await
        .context("couldn't run git rev-parse --verify")?;
    Ok(verify.success().then_some(head))
}

/// Full sha of the commit `rev` points at (tags dereferenced via `^{commit}`),
/// or `None` if `rev` doesn't resolve to anything in this repo — used to
/// verify a web-submitted tag target actually names an existing commit
/// before it's trusted enough to write into the manifest.
pub async fn resolve_commit(git_dir: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["rev-parse", "--verify", "-q", "--end-of-options", &spec])
        .output()
        .await
        .context("couldn't run git rev-parse --verify")?;
    if !out.status.success() {
        return Ok(None);
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!sha.is_empty()).then_some(sha))
}
