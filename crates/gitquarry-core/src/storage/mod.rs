pub mod audit;
pub mod commit_sig;
pub mod entities;
pub mod lfs_locks;
pub mod pull_requests;
pub mod remote;
pub mod search_index;
pub mod settings;
pub mod thread;
pub mod uid;

use crate::config::BucketConfig;
use std::sync::Arc;

/// Every repo lives under this top-level S3 prefix — `repos/<owner>/<name>`,
/// not `<owner>/<name>` at the bucket root — so a repo's namespace (a
/// username or group, both fairly free-form) can never collide with `.conf/`
/// (server-wide config) or any other reserved bucket-root folder: the two
/// live under disjoint prefixes by construction, not by a name blocklist.
pub const REPOS_PREFIX: &str = "repos";

/// Shared root for every bucket-side lookup/hint index — pubkey, email,
/// namespace (in `auth::access`), plus repo-name and reverse uid indexes
/// (in `storage::settings`). Lives here, not in `auth::access`, because
/// more than one module needs it.
pub const INDEX_PREFIX: &str = ".conf/index";

// The storage backend abstraction (`Backend`, `Cas`, `build_backend`) and
// the encrypted-JSON primitives (`get_json`/`put_json`/etc) now live in the
// `s3db` crate — re-exported here so every existing call site keeps working
// unchanged.
pub use s3db::{Backend, Cas, get_json, list_all_keys, put_json};

/// Builds the right [`Backend`] for `cfg` — the single place that turns
/// local `gitquarry.toml` config into a live storage connection.
pub fn build_backend(cfg: &BucketConfig) -> Arc<dyn Backend> {
    s3db::build_backend(&cfg.backend)
}

/// Delay before a CAS-conflict retry, exponential (capped) with a little
/// jitter — every counter/get-modify-put retry loop in this codebase used to
/// retry immediately with no delay at all (`storage::issues::next_issue_number`
/// and its half-dozen siblings), which just means every contending writer
/// hammers the same key back-to-back under real contention instead of
/// spreading out. Deliberately small: this smooths out a handful of
/// concurrent writers, it isn't real backpressure — a loop that still
/// exhausts its attempt cap under this delay was going to fail either way.
pub async fn cas_retry_backoff(attempt: u32) {
    use std::time::Duration;
    const BASE: Duration = Duration::from_millis(15);
    const MAX: Duration = Duration::from_millis(400);
    let backoff = BASE.saturating_mul(1u32 << attempt.min(6)).min(MAX);
    let jitter = Duration::from_millis(rand::random::<u64>() % 15);
    tokio::time::sleep(backoff + jitter).await;
}

/// Test-only shim: `s3db::LocalBackend` under its old path, so existing
/// `#[cfg(test)]` helpers across the tree don't all need editing for a
/// module that only moved crates, not shape. `feature = "test-util"` also
/// enables it: `gitquarry-webui`'s own tests build the same kind of local
/// test fixture, and `#[cfg(test)]` alone only applies to this crate's own
/// test build, not a downstream crate's.
#[cfg(any(test, feature = "test-util"))]
pub mod local {
    pub use s3db::LocalBackend;
}
