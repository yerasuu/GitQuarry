//! The publish filter (`docs/publish-mirror-concept.md` §2): which tree
//! entries cross to the public side.
//!
//! Two sources, deliberately asymmetric. The mirror entity is authoritative
//! and can say anything; `.gitquarry/publish.toml` in the source ref is
//! writable by anyone who can push to the private repo, so it can only add
//! exclusions — the effective filter is the union, and nothing in the file
//! can make a path public that the entity alone would keep private.
//!
//! Patterns use `.gitignore` glob semantics against the full path from the
//! tree root, and are evaluated per entry while walking the tree, so a
//! directory that is excluded is dropped before its children are read.

use crate::validate::validate_publish_patterns;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The path of the in-repo half of the filter, relative to the tree root.
pub const REPO_FILTER_PATH: &str = ".gitquarry/publish.toml";

/// Largest `.gitquarry/publish.toml` read. A file this size is already far
/// past any real filter; past it the file is refused, not truncated, since a
/// truncated filter is a narrower one.
pub const MAX_REPO_FILTER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    /// Only what `include` names is published: a new top-level directory
    /// stays private until somebody lists it. The recommended mode.
    #[default]
    Allowlist,
    /// Everything is published except what `exclude` names.
    Exclude,
}

/// The mirror entity's half of the filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    #[serde(default)]
    pub mode: FilterMode,
    /// Read only in [`FilterMode::Allowlist`].
    #[serde(default)]
    pub include: Vec<String>,
    /// Applies in both modes, and wins over `include`.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Gitlinks point at other repos, which may be private themselves.
    #[serde(default = "default_true")]
    pub exclude_submodules: bool,
    /// Drops a directory the filter left empty. With it off, an emptied
    /// directory is kept as an empty tree, so its *name* is still published.
    #[serde(default = "default_true")]
    pub strip_empty_dirs: bool,
}

fn default_true() -> bool {
    true
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            mode: FilterMode::default(),
            include: Vec::new(),
            exclude: Vec::new(),
            exclude_submodules: true,
            strip_empty_dirs: true,
        }
    }
}

/// `.gitquarry/publish.toml`. Unknown keys are an error rather than ignored:
/// a contributor who writes `include = [...]` or `mode = ...` there expects
/// it to do something, and silently dropping the line would publish what
/// they meant to hold back. Neither key exists here, because either one
/// could widen what is published.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoFilterFile {
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Can only turn the rule on: `false` here leaves the entity's setting.
    #[serde(default)]
    pub exclude_submodules: bool,
    /// Can only turn the rule on, like `exclude_submodules`.
    #[serde(default)]
    pub strip_empty_dirs: bool,
}

impl RepoFilterFile {
    /// # Errors
    ///
    /// Refuses a file that isn't UTF-8, isn't valid TOML, carries a key this
    /// file doesn't accept, or a pattern [`validate_publish_patterns`]
    /// rejects.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_REPO_FILTER_BYTES {
            anyhow::bail!("{REPO_FILTER_PATH} is larger than {MAX_REPO_FILTER_BYTES} bytes");
        }
        let text = std::str::from_utf8(bytes).with_context(|| format!("{REPO_FILTER_PATH} is not UTF-8"))?;
        let file: Self = toml::from_str(text).with_context(|| format!("{REPO_FILTER_PATH} is not valid"))?;
        validate_publish_patterns(&file.exclude).context(REPO_FILTER_PATH)?;
        Ok(file)
    }
}

/// Why an entry was left out of the public tree — what the preview groups
/// excluded paths by.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExcludedBy {
    /// A pattern from the mirror entity.
    Config(String),
    /// A pattern from `.gitquarry/publish.toml`.
    RepoFile(String),
    /// The filter file itself: the list of what is private is not public.
    FilterFile,
    Submodule,
    /// Allowlist mode, and no `include` pattern names the entry.
    NotInAllowlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Gitlink,
}

/// What the filter says about one tree entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Published. `included` is whether the entry is inside the allowlist
    /// itself (always `true` in exclude mode), which a directory's children
    /// inherit.
    Keep {
        included: bool,
    },
    Exclude(ExcludedBy),
}

/// The effective filter: entity config ∪ the in-repo file.
#[derive(Debug, Clone)]
pub struct Filter {
    mode: FilterMode,
    include: Vec<Pattern>,
    exclude: Vec<(ExcludedBy, Pattern)>,
    exclude_submodules: bool,
    strip_empty_dirs: bool,
}

impl Filter {
    /// The union of both sources. `repo_file` is `None` when the source ref
    /// has no `.gitquarry/publish.toml`.
    pub fn new(config: &FilterConfig, repo_file: Option<&RepoFilterFile>) -> Self {
        let mut exclude: Vec<(ExcludedBy, Pattern)> = config
            .exclude
            .iter()
            .map(|raw| (ExcludedBy::Config(raw.clone()), Pattern::new(raw)))
            .collect();
        if let Some(file) = repo_file {
            exclude.extend(
                file.exclude
                    .iter()
                    .map(|raw| (ExcludedBy::RepoFile(raw.clone()), Pattern::new(raw))),
            );
        }
        Self {
            mode: config.mode,
            include: config.include.iter().map(|raw| Pattern::new(raw)).collect(),
            exclude,
            exclude_submodules: config.exclude_submodules || repo_file.is_some_and(|f| f.exclude_submodules),
            strip_empty_dirs: config.strip_empty_dirs || repo_file.is_some_and(|f| f.strip_empty_dirs),
        }
    }

    pub fn strip_empty_dirs(&self) -> bool {
        // An emptied directory in allowlist mode was never named by the
        // allowlist, so keeping its name would publish something nobody
        // listed.
        self.strip_empty_dirs || self.mode == FilterMode::Allowlist
    }

    /// The verdict on one entry at `path` (full path from the tree root, no
    /// leading `/`), given whether its parent directory was itself inside the
    /// allowlist.
    ///
    /// A directory is excluded, and so never read, when an exclude rule
    /// matches it, or in allowlist mode when no include pattern could match
    /// anything under it.
    pub fn decide(&self, path: &str, kind: EntryKind, parent_included: bool) -> Decision {
        if kind == EntryKind::Gitlink && self.exclude_submodules {
            return Decision::Exclude(ExcludedBy::Submodule);
        }
        if kind != EntryKind::Dir && path == REPO_FILTER_PATH {
            return Decision::Exclude(ExcludedBy::FilterFile);
        }
        let is_dir = kind == EntryKind::Dir;
        if let Some((by, _)) = self.exclude.iter().find(|(_, p)| p.matches(path, is_dir)) {
            return Decision::Exclude(by.clone());
        }
        if self.mode == FilterMode::Exclude || parent_included {
            return Decision::Keep { included: true };
        }
        if self.include.iter().any(|p| p.matches(path, is_dir)) {
            return Decision::Keep { included: true };
        }
        if is_dir && self.include.iter().any(|p| p.could_match_under(path)) {
            return Decision::Keep { included: false };
        }
        Decision::Exclude(ExcludedBy::NotInAllowlist)
    }

    /// Whether `path` — a file in a tree that is about to be published — is
    /// one the filter says must stay private, checking every ancestor
    /// directory independently of how the tree was built. This is the
    /// filter audit's own check, so it deliberately doesn't reuse the walk's
    /// pruning: it exists to catch a bug there.
    pub fn excludes_published_path(&self, path: &str, kind: EntryKind) -> Option<ExcludedBy> {
        let mut parent_included = false;
        for (i, _) in path.match_indices('/') {
            match self.decide(&path[..i], EntryKind::Dir, parent_included) {
                Decision::Exclude(by) => return Some(by),
                Decision::Keep { included } => parent_included = included,
            }
        }
        match self.decide(path, kind, parent_included) {
            Decision::Exclude(by) => Some(by),
            Decision::Keep { .. } => None,
        }
    }
}

/// One compiled `.gitignore`-style pattern.
#[derive(Debug, Clone)]
struct Pattern {
    segments: Vec<String>,
    /// No `/` except a trailing one: matches the last path segment at any
    /// depth, as `.gitignore` does.
    basename_only: bool,
    dir_only: bool,
    /// Ends in `/**`. `.gitignore` reads that as "everything inside" and not
    /// the directory itself; it is also taken to match the directory here,
    /// so the directory is dropped whole instead of surviving as an empty
    /// tree that still publishes its name when `strip_empty_dirs` is off.
    /// The published file set is the same either way.
    contents_of_dir: bool,
}

impl Pattern {
    fn new(raw: &str) -> Self {
        let dir_only = raw.ends_with('/');
        let trimmed = raw.trim_end_matches('/');
        let basename_only = !trimmed.contains('/');
        let trimmed = trimmed.strip_prefix('/').unwrap_or(trimmed);
        let segments: Vec<String> = trimmed.split('/').map(str::to_string).collect();
        let contents_of_dir = segments.len() > 1 && segments.last().is_some_and(|s| s == "**");
        Self {
            segments,
            basename_only,
            dir_only,
            contents_of_dir,
        }
    }

    fn matches(&self, path: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        if self.basename_only {
            let base = path.rsplit('/').next().unwrap_or(path);
            return glob_segment(self.segments[0].as_bytes(), base.as_bytes());
        }
        let path_segs: Vec<&str> = path.split('/').collect();
        if match_segments(&self.segments, &path_segs) {
            return true;
        }
        is_dir && self.contents_of_dir && match_segments(&self.segments[..self.segments.len() - 1], &path_segs)
    }

    /// Whether some path strictly under the directory `dir` could match —
    /// what decides if allowlist mode has to read the directory at all.
    fn could_match_under(&self, dir: &str) -> bool {
        if self.basename_only {
            return true;
        }
        let mut pattern = self.segments.as_slice();
        for seg in dir.split('/') {
            match pattern.first().map(String::as_str) {
                None => return false,
                Some("**") => return true,
                Some(p) if glob_segment(p.as_bytes(), seg.as_bytes()) => pattern = &pattern[1..],
                Some(_) => return false,
            }
        }
        !pattern.is_empty()
    }
}

/// `**` as a whole segment spans zero or more path segments.
fn match_segments(pattern: &[String], path: &[&str]) -> bool {
    match pattern.first().map(String::as_str) {
        None => path.is_empty(),
        Some("**") => match_segments(&pattern[1..], path) || (!path.is_empty() && match_segments(pattern, &path[1..])),
        Some(seg) => {
            !path.is_empty()
                && glob_segment(seg.as_bytes(), path[0].as_bytes())
                && match_segments(&pattern[1..], &path[1..])
        }
    }
}

/// `*`, `?`, `[...]` (with `!`/`^` negation and ranges) and `\` escapes,
/// within one path segment — the `fnmatch` subset `.gitignore` documents.
fn glob_segment(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                b'*' => {
                    while p < pattern.len() && pattern[p] == b'*' {
                        p += 1;
                    }
                    backtrack = Some((p, t));
                    continue;
                }
                b'?' => {
                    p += 1;
                    t += 1;
                    continue;
                }
                b'[' => {
                    if let Some((matched, next)) = match_class(pattern, p, text[t]) {
                        if matched {
                            p = next;
                            t += 1;
                            continue;
                        }
                    } else if text[t] == b'[' {
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
                b'\\' if p + 1 < pattern.len() => {
                    if pattern[p + 1] == text[t] {
                        p += 2;
                        t += 1;
                        continue;
                    }
                }
                c => {
                    if c == text[t] {
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
            }
        }
        match backtrack {
            Some((bp, bt)) => {
                p = bp;
                t = bt + 1;
                backtrack = Some((bp, bt + 1));
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// `[...]` starting at `pattern[start]`: `Some((matched, index after the
/// closing bracket))`, or `None` for an unterminated class, which is then a
/// literal `[`.
fn match_class(pattern: &[u8], start: usize, c: u8) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negated = matches!(pattern.get(i), Some(b'!' | b'^'));
    if negated {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        let mut lo = pattern[i];
        if lo == b']' && !first {
            return Some((matched != negated, i + 1));
        }
        first = false;
        if lo == b'\\' && i + 1 < pattern.len() {
            i += 1;
            lo = pattern[i];
        }
        if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            let hi = pattern[i + 2];
            matched |= lo <= c && c <= hi;
            i += 3;
        } else {
            matched |= lo == c;
            i += 1;
        }
    }
    None
}
