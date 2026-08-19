//! Per-entry "last commit that touched it" for a tree listing — one
//! `git log --name-status` walk instead of one `git log -1 -- <entry>`
//! subprocess per row, which wouldn't scale to a directory with many entries.

use super::log::{CommitInfo, LOG_FORMAT, LOG_SEP, parse_log_line};
use crate::git::run_text;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;

/// How many commits of history `git log` itself is allowed to walk looking
/// for matches, regardless of how many `entries` are still unresolved.
/// `Command::output()` buffers the whole child process before this function
/// ever sees a line of it, so an early break on the Rust side alone doesn't
/// bound the git-side cost on a repo whose relevant directory hasn't
/// changed in a very long history — this cap does. An entry not found
/// within the cap is simply absent from the result; callers already treat
/// "no entry" as "unknown", the same as any other missing history.
const HISTORY_CAP: usize = 400;

/// For each name in `entries` (the immediate children of `path`, as
/// returned by [`super::ls_tree`]), the most recent commit whose changes
/// touched it — a file changed directly, or any file underneath it if it's
/// a directory. An entry can be missing from the result if nothing in the
/// last [`HISTORY_CAP`] commits touched it.
pub async fn last_touched(
    git_dir: &Path,
    rev: &str,
    path: &str,
    entries: &[String],
) -> Result<HashMap<String, CommitInfo>> {
    let mut pending: HashSet<&str> = entries.iter().map(String::as_str).collect();
    let mut resolved = HashMap::with_capacity(entries.len());
    if pending.is_empty() {
        return Ok(resolved);
    }

    let scoped = !path.is_empty();
    let limit_flag = format!("-n{HISTORY_CAP}");
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("log"),
        OsStr::new(LOG_FORMAT),
        // Same `YYYY/MM/DD` display format `log_args` uses -- this walk
        // feeds the tree view's own "last touched" date column.
        OsStr::new("--date=format:%Y/%m/%d"),
        OsStr::new("--name-status"),
        OsStr::new(&limit_flag),
        OsStr::new("--end-of-options"),
        OsStr::new(rev),
    ];
    if scoped {
        args.push(OsStr::new("--"));
        args.push(OsStr::new(path));
    }
    let out = run_text(Some(git_dir), &args).await?;

    let prefix = if scoped { format!("{path}/") } else { String::new() };
    let mut current: Option<CommitInfo> = None;
    for line in out.lines() {
        if pending.is_empty() {
            break;
        }
        // Header lines come straight from `LOG_FORMAT`, which joins fields
        // with `LOG_SEP` (a control byte that can't occur in a normal
        // "<status>\t<path>" line) — that alone tells the two apart.
        if line.contains(LOG_SEP) {
            current = parse_log_line(line);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let Some(commit) = current.as_ref() else { continue };
        // "<status>\t<path>" for a plain change, "<status>\t<old>\t<new>"
        // for a rename/copy — either way the last field is the path this
        // commit left behind, which is what a still-current entry matches.
        let Some(changed) = line.rsplit('\t').next() else {
            continue;
        };
        let rel = if scoped {
            match changed.strip_prefix(&prefix) {
                Some(r) => r,
                None => continue,
            }
        } else {
            changed
        };
        let child = rel.split('/').next().unwrap_or(rel);
        if pending.remove(child) {
            resolved.insert(child.to_string(), commit.clone());
        }
    }
    Ok(resolved)
}
