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
