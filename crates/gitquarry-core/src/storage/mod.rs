pub mod audit;
pub mod commit_sig;
pub mod entities;
pub mod lfs_locks;
pub mod merge_requests;
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
/// local `gitquarry.toml` config into a live storage connection. `Err` for
/// a driver section the engine can't construct (a malformed endpoint, an
/// unreadable local path); the caller reports it against the config file it
/// came from.
///
/// Wraps the raw driver in `s3db`'s resilience stack (concurrency caps,
/// retry/backoff, circuit breaker, cross-request LRU cache — one call per
/// process, so its cache and circuit-breaker state are shared by every
/// request, not rebuilt per request) rather than handing back the bare
/// driver: this is the only place that stack gets attached, so a caller
/// can't accidentally bypass it by calling `s3db::build_backend` directly.
/// Metrics are a [`s3db::NoopMetrics`] sink for now — nothing in gitquarry
/// reads `MeteredBackend`'s counters yet.
pub fn build_backend(cfg: &BucketConfig) -> anyhow::Result<Arc<dyn Backend>> {
    let raw = s3db::build_backend(&cfg.backend)?;
    Ok(s3db::build_resilient_backend(
        raw,
        cfg.resilience.clone(),
        std::sync::Arc::new(s3db::NoopMetrics),
    ))
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
