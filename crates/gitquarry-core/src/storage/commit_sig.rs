//! Cached result of verifying one commit's GPG signature against every
//! registered account's `auth::entities::User::gpg_keys` — see
//! [`crate::domain::commit_signature::verified_signer`], the only writer.
//!
//! Global (a fixed `#[entity(collection)]`, not a per-repo `Collection`
//! like [`super::issues`]/[`super::lfs_locks`] need): a commit sha is
//! content-addressed, so the same sha always means the exact same bytes —
//! the same signature, the same verification result — everywhere it shows
//! up, mirror or fork included. Scoping this per-repo would just mean
//! re-verifying (and re-storing) the identical answer once per repo that
//! happens to share history, for no benefit.

use s3db::Entity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, Entity)]
#[entity(collection = ".conf/commit-sig")]
pub struct CommitSigCache {
    #[serde(default)]
    #[entity(key)]
    pub sha: String,
    /// `None`: unsigned, or signed but no registered account's `gpg_keys`
    /// verifies it. `Some(username)`: that account's own key verified it —
    /// same "author-email-first candidate, then GPG-verify" resolution
    /// `webui::repo::commit::registered_key_signer` used before this cache
    /// existed.
    #[serde(default)]
    pub verified_username: Option<String>,
}
