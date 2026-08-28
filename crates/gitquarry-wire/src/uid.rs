//! Stable, immutable identifier assigned to a Repo/User/Group at creation
//! time and never changed afterward — what every cross-entity reference
//! (`RepoConfig::owner`, `User::starred`, `Group::owners`, ...) stores
//! instead of a rename-able display name, so a rename/move never leaves a
//! dangling or hijacked-by-name-reuse reference elsewhere. One shared type
//! across all three entity kinds rather than three phantom-typed newtypes,
//! since this codebase doesn't parameterize IDs by kind anywhere else and a
//! misplaced user-uid-where-a-repo-uid-is-expected is a logic bug either
//! way. Lives in `gitquarry-wire`, not `gitquarry-core`, because a repo/MR
//! reference crosses the core/tracker IPC boundary and needs to be the
//! identical type on both sides.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct Uid(String);

impl Uid {
    /// 128 bits of `rand::random`, lowercase hex — same technique the
    /// audit log's random suffix already uses, just wide enough to stay
    /// unique for an entity's entire lifetime rather than one process run.
    pub fn new() -> Self {
        Uid(format!("{:032x}", rand::random::<u128>()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True for `Uid::default()` — the "no claim"/"field not set yet"
    /// sentinel used by pointer structs like `RepoUidPointer`, mirroring
    /// `UsernamePointer { username: String }`'s own empty-string-means-
    /// nothing convention.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reconstructs a `Uid` from its own [`fmt::Display`]ed text — for reading one
/// back from somewhere it was previously written verbatim (an S3 key
/// segment, a local mirror directory name), never for parsing arbitrary
/// input: nothing validates the format here, callers already trust the
/// source.
impl From<String> for Uid {
    fn from(raw: String) -> Self {
        Uid(raw)
    }
}
