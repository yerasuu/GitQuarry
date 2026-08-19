//! Line-by-line blame.

use crate::git::run_text;
use anyhow::Result;
use std::ffi::OsStr;
use std::path::Path;

#[derive(Debug)]
pub struct BlameLine {
    pub sha: String,
    pub author: String,
    pub content: String,
}

/// Line-by-line blame of `rev:path`, parsed from `--porcelain` output. Each
/// porcelain record starts with a header line (`<sha> <orig-line> <final-line>
/// [<num-lines>]`), then metadata lines, then a line starting with a tab
/// holding the actual content — we only keep sha/author/content per line.
///
/// No `--end-of-options`: unlike every other command in this module, `git
/// blame` doesn't recognize it and misparses `rev` as a path instead
/// (`fatal: bad revision`). Safe without it regardless — the caller only
/// ever passes a `rev`/`path` that already went through
/// [`crate::validate::validate_rev`]/[`crate::validate::validate_tree_path`], both of
/// which reject a leading `-`.
pub async fn blame(git_dir: &Path, rev: &str, path: &str) -> Result<Vec<BlameLine>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("blame"),
            OsStr::new("--porcelain"),
            OsStr::new(rev),
            OsStr::new("--"),
            OsStr::new(path),
        ],
    )
    .await?;
    Ok(parse_blame_porcelain(&out))
}

// Already reachable unconditionally from `blame()` above, so no test-only
// cfg gate is needed here — just widen to `pub` for the integration test.
pub fn parse_blame_porcelain(text: &str) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut cur_sha = String::new();
    let mut cur_author = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('\t') {
            lines.push(BlameLine {
                sha: cur_sha.clone(),
                author: cur_author.clone(),
                content: rest.to_string(),
            });
        } else if let Some(author) = line.strip_prefix("author ") {
            cur_author = author.to_string();
        } else if let Some(sha) = line.split(' ').next()
            && sha.len() == 40
            && sha.bytes().all(|b| b.is_ascii_hexdigit())
        {
            cur_sha = sha.to_string();
        }
    }
    lines
}
