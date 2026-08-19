use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A public repo's metadata as handed to a peer — unifies what used to be
/// two near-identical structs (one serialized by `/_api/repos`, one
/// deserialized by the federation refresh client fetching it from a peer).
/// No `default_branch`/`updated_at`: `RepoConfig` doesn't actually store
/// either (`default_branch` lives in the git ref data itself, `updated_at`
/// isn't tracked anywhere), so this only serializes fields that are real
/// today rather than fabricating placeholders for the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSummary {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub topics: BTreeSet<String>,
    #[serde(default)]
    pub star_count: u32,
    #[serde(default)]
    pub fork_count: u32,
}
