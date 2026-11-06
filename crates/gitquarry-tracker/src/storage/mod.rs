//! Issue/board/milestone storage — same `s3db::Collection` convention
//! `gitquarry_core::storage::pull_requests` uses (per-repo-uid-scoped
//! metadata, `Collection::new_owned` rather than `#[derive(Entity)]`, which
//! needs a compile-time-literal collection name).

pub mod board;
pub mod issues;
pub mod milestones;
pub mod thread;

/// Byte-identical to `gitquarry_core::storage::REPOS_PREFIX` — a fixed
/// literal, never going to drift, not worth a shared crate for one string
/// (`docs/tracker-binary-split-concept.md`'s own reasoning).
pub const REPOS_PREFIX: &str = "repos";

/// Duplicated from `gitquarry_core::storage::cas_retry_backoff` — same
/// "cheap to duplicate a few lines" reasoning as [`thread`].
pub async fn cas_retry_backoff(attempt: u32) {
    use std::time::Duration;
    const BASE: Duration = Duration::from_millis(15);
    const MAX: Duration = Duration::from_millis(400);
    let backoff = BASE.saturating_mul(1u32 << attempt.min(6)).min(MAX);
    let jitter = Duration::from_millis(rand::random::<u64>() % 15);
    tokio::time::sleep(backoff + jitter).await;
}
