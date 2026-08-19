//! `.gitmodules` lookup, to link gitlink tree entries to their remote.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

/// `path -> url` entries from `.gitmodules` at `rev`, to link gitlink tree
/// entries to their submodule remote instead of treating them as blobs.
/// Returns an empty map if the repo has no `.gitmodules` at that rev.
pub async fn read_gitmodules(git_dir: &Path, rev: &str) -> Result<HashMap<String, String>> {
    let spec = format!("{rev}:.gitmodules");
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["cat-file", "-p", "--end-of-options", &spec])
        .output()
        .await
        .context("could not run git cat-file for .gitmodules")?;
    if !out.status.success() {
        return Ok(HashMap::new());
    }
    Ok(parse_gitmodules(&String::from_utf8_lossy(&out.stdout)))
}

/// Minimal `.gitmodules` INI parser: only cares about `path` and `url` keys
/// inside `[submodule "..."]` sections, matched up when a new section starts
/// or the file ends. `pub` (not cfg-gated, unlike most test-only widenings
/// here) because `read_gitmodules` below calls it unconditionally in
/// production too — the moved integration tests just reach the same fn.
pub fn parse_gitmodules(text: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut cur_path: Option<String> = None;
    let mut cur_url: Option<String> = None;
    let flush = |path: &mut Option<String>, url: &mut Option<String>, out: &mut HashMap<String, String>| {
        if let (Some(p), Some(u)) = (path.take(), url.take()) {
            out.insert(p, u);
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            flush(&mut cur_path, &mut cur_url, &mut result);
        } else if let Some(v) = line.strip_prefix("path")
            && let Some(v) = v.trim_start().strip_prefix('=')
        {
            cur_path = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("url")
            && let Some(v) = v.trim_start().strip_prefix('=')
        {
            cur_url = Some(v.trim().to_string());
        }
    }
    flush(&mut cur_path, &mut cur_url, &mut result);
    result
}
