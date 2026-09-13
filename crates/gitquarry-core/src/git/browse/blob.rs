//! Blob size/content lookup.

use crate::git::{run, run_text};
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::path::Path;

/// Blob size at `rev:path`, without pulling it into memory.
pub async fn blob_size(git_dir: &Path, rev: &str, path: &str) -> Result<u64> {
    let spec = format!("{rev}:{path}");
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("cat-file"),
            OsStr::new("-s"),
            OsStr::new("--end-of-options"),
            OsStr::new(&spec),
        ],
    )
    .await?;
    out.parse()
        .with_context(|| format!("size of {spec} isn't a number: {out}"))
}

/// `(blob sha, size in bytes)` for `rev:path` — one `ls-tree -l`, the same
/// single round trip [`blob_size`] costs, and the sha is what lets a caller
/// cache anything derived from these bytes: a blob sha *is* the content, so
/// the key can never go stale.
pub async fn blob_id(git_dir: &Path, rev: &str, path: &str) -> Result<(String, u64)> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("ls-tree"),
            OsStr::new("-l"),
            OsStr::new("--end-of-options"),
            OsStr::new(rev),
            OsStr::new("--"),
            OsStr::new(path),
        ],
    )
    .await?;
    // `<mode> <type> <sha> <size>\t<path>`, and empty output for a path
    // that isn't in this tree — `ls-tree` reports that by saying nothing
    // rather than failing, so it has to be caught here.
    let (meta, _) = out
        .split_once('\t')
        .with_context(|| format!("{rev}:{path} is not in this tree"))?;
    let mut fields = meta.split_whitespace();
    let kind = fields.nth(1).unwrap_or_default();
    if kind != "blob" {
        anyhow::bail!("{rev}:{path} is a {kind}, not a blob");
    }
    let sha = fields.next().unwrap_or_default().to_string();
    let size = fields
        .next()
        .unwrap_or_default()
        .parse()
        .with_context(|| format!("size of {rev}:{path} isn't a number: {meta}"))?;
    Ok((sha, size))
}

/// Raw bytes of a blob at `rev:path`.
pub async fn read_blob(git_dir: &Path, rev: &str, path: &str) -> Result<Vec<u8>> {
    let spec = format!("{rev}:{path}");
    run(
        Some(git_dir),
        &[
            OsStr::new("cat-file"),
            OsStr::new("-p"),
            OsStr::new("--end-of-options"),
            OsStr::new(&spec),
        ],
    )
    .await
}
