//! Tree listing, filename search, and per-language byte stats.

use crate::git::run_text;
use anyhow::Result;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

#[derive(Debug)]
pub struct TreeEntry {
    pub kind: String,
    pub sha: String,
    pub name: String,
}

fn tree_spec(rev: &str, path: &str) -> String {
    if path.is_empty() {
        rev.to_string()
    } else {
        format!("{rev}:{path}")
    }
}

/// Contents of a tree at `rev` (root if `path` is empty).
pub async fn ls_tree(git_dir: &Path, rev: &str, path: &str) -> Result<Vec<TreeEntry>> {
    let spec = tree_spec(rev, path);
    let out = run_text(
        Some(git_dir),
        &[OsStr::new("ls-tree"), OsStr::new("--end-of-options"), OsStr::new(&spec)],
    )
    .await?;

    let mut entries = Vec::new();
    for line in out.lines() {
        // "<mode> <type> <sha>\t<name>"
        let Some((meta, name)) = line.split_once('\t') else {
            continue;
        };
        let mut meta_parts = meta.split(' ');
        let Some(kind) = meta_parts.nth(1) else {
            continue;
        };
        let Some(sha) = meta_parts.next() else {
            continue;
        };
        entries.push(TreeEntry {
            kind: kind.to_string(),
            sha: sha.to_string(),
            name: name.to_string(),
        });
    }
    Ok(entries)
}

/// Every file path in the tree at `rev` (recursive, unlike [`ls_tree`]'s
/// single level) whose name contains `query`, case-insensitively — one
/// `git ls-tree -r` per call, filtered in-process rather than via `git`'s
/// own pathspec matching, so `query` is a plain substring, not a glob the
/// caller has to escape. `query` itself is never validated here — same
/// convention as [`super::log::search_log`], the caller runs it through
/// `validate::validate_query` first.
pub async fn search_filenames(git_dir: &Path, rev: &str, query: &str) -> Result<Vec<String>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("ls-tree"),
            OsStr::new("-r"),
            OsStr::new("--name-only"),
            OsStr::new("--end-of-options"),
            OsStr::new(rev),
        ],
    )
    .await?;
    let query_lower = query.to_ascii_lowercase();
    Ok(out
        .lines()
        .filter(|name| name.to_ascii_lowercase().contains(&query_lower))
        .map(str::to_string)
        .collect())
}

/// A bounded extension→language table, not full github-linguist parity —
/// covers common languages; anything else contributes to no language at
/// all rather than an "other" bucket (a handful of misc extensions
/// wouldn't render meaningfully on a bar this narrow anyway). Lowercase,
/// no leading dot.
fn language_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "Rust",
        "py" => "Python",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" => "TypeScript",
        "tsx" => "TSX",
        "jsx" => "JSX",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "C++",
        "cs" => "C#",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "scala" => "Scala",
        "hs" => "Haskell",
        "ex" | "exs" => "Elixir",
        "erl" => "Erlang",
        "lua" => "Lua",
        "pl" | "pm" => "Perl",
        "sh" | "bash" | "zsh" => "Shell",
        "ps1" => "PowerShell",
        "sql" => "SQL",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "md" | "markdown" => "Markdown",
        "dockerfile" => "Dockerfile",
        "nix" => "Nix",
        "zig" => "Zig",
        "dart" => "Dart",
        "r" => "R",
        "jl" => "Julia",
        _ => return None,
    })
}

/// Approximate per-language byte totals across the tree at `rev`, sorted
/// by size descending — one `git ls-tree -r -l` (`-l` already gives each
/// blob's size, unlike [`ls_tree`]/[`search_filenames`], so there's no
/// second `cat-file`/[`super::blob::blob_size`] call per file). See
/// [`language_for_extension`] for what counts as a "language" here.
pub async fn language_stats(git_dir: &Path, rev: &str) -> Result<Vec<(String, u64)>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("ls-tree"),
            OsStr::new("-r"),
            OsStr::new("-l"),
            OsStr::new("--end-of-options"),
            OsStr::new(rev),
        ],
    )
    .await?;
    let mut totals: HashMap<&'static str, u64> = HashMap::new();
    for line in out.lines() {
        // "<mode> <type> <sha> <size>\t<path>"
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let parts: Vec<&str> = meta.split_whitespace().collect();
        if parts.len() < 4 || parts[1] != "blob" {
            continue;
        }
        let Some(size) = parts[3].parse::<u64>().ok() else {
            continue;
        };
        let Some(ext) = path.rsplit('.').next().filter(|e| *e != path) else {
            continue;
        };
        let Some(lang) = language_for_extension(&ext.to_ascii_lowercase()) else {
            continue;
        };
        *totals.entry(lang).or_insert(0) += size;
    }
    let mut totals: Vec<(String, u64)> = totals
        .into_iter()
        .map(|(lang, size)| (lang.to_string(), size))
        .collect();
    totals.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(totals)
}
