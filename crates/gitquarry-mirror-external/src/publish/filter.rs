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

use serde::{Deserialize, Serialize};

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
