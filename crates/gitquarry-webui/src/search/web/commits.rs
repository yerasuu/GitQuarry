//! Commit-message half of `/_search` — reads each local repo's own stored
//! `gitquarry_core::storage::search_index::SearchIndex::commits` (see that module's
//! own doc comment) instead of running `git log --grep` live; falls back
//! to the live [`gitquarry_core::git::browse::search_log`] path for a repo with no
//! index yet, or a peer mirror — same fallback shape
//! [`super::filenames::search_all`] uses.

use gitquarry_core::app::AppContext;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::task::JoinSet;

pub struct CommitHit {
    pub repo: String,
    pub sha: String,
    pub subject: String,
}

/// Every commit-message match for `query` across `repo_names`, sorted by
/// repo name for a deterministic page, capped at [`super::MAX_RESULTS`]
/// total — the `bool` is whether the cap actually cut something off. Same
/// fail-open-per-repo reasoning as [`super::filenames::search_all`].
/// `(sha, subject)` hits keyed by repo name, as [`search_all`] accumulates
/// them before flattening.
type HitsByRepo = BTreeMap<String, Vec<(String, String)>>;

/// One finished per-repo scan, as `JoinSet::join_next` yields it.
type ScanResult = Result<(String, Option<Vec<(String, String)>>), tokio::task::JoinError>;

/// Folds one finished scan into the accumulator — a repo that failed, was
/// cancelled, or simply matched nothing contributes no entry, the
/// fail-open-per-repo behavior [`search_all`] documents.
fn collect(result: ScanResult, by_repo: &mut HitsByRepo) {
    if let Ok((repo, Some(commits))) = result
        && !commits.is_empty()
    {
        by_repo.insert(repo, commits);
    }
}

pub async fn search_all(ctx: &Arc<AppContext>, repo_names: &[String], query: &str) -> (Vec<CommitHit>, bool) {
    let mut set = JoinSet::new();
    let mut by_repo: HitsByRepo = BTreeMap::new();
    for repo in repo_names {
        // Drain one before starting the next once the window is full, so
        // this never holds more than `MAX_CONCURRENT_REPO_SCANS` scans in
        // flight regardless of how many repos there are — see that
        // constant for why that ceiling exists.
        if set.len() >= super::MAX_CONCURRENT_REPO_SCANS
            && let Some(result) = set.join_next().await
        {
            collect(result, &mut by_repo);
        }
        let ctx = Arc::clone(ctx);
        let repo = repo.clone();
        let query = query.to_string();
        set.spawn(async move { (repo.clone(), search_one(&ctx, &repo, &query).await) });
    }
    while let Some(result) = set.join_next().await {
        collect(result, &mut by_repo);
    }
    let mut all: Vec<CommitHit> = by_repo
        .into_iter()
        .flat_map(|(repo, commits)| {
            commits.into_iter().map(move |(sha, subject)| CommitHit {
                repo: repo.clone(),
                sha,
                subject,
            })
        })
        .collect();
    let truncated = all.len() > super::MAX_RESULTS;
    all.truncate(super::MAX_RESULTS);
    (all, truncated)
}

/// `(sha, subject)` pairs — the only two fields [`CommitHit`] actually
/// needs, so the stored-index and live-fallback paths can share one return
/// shape without dragging in the live path's full `git::browse::CommitInfo`.
async fn search_one(ctx: &AppContext, repo: &str, query: &str) -> Option<Vec<(String, String)>> {
    if let Some(uid) = super::repo_uid_if_local(ctx, repo).await
        && let Ok((Some(index), _etag)) =
            gitquarry_core::storage::search_index::get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid).await
        && index.last_indexed_sha.is_some()
    {
        let query_lower = query.to_ascii_lowercase();
        let hits = index
            .commits
            .into_iter()
            .filter(|c| c.subject.to_ascii_lowercase().contains(&query_lower))
            .take(super::MAX_RESULTS)
            .map(|c| (c.sha, c.subject))
            .collect();
        return Some(hits);
    }
    // No stored index (never pushed to since this feature shipped, or a
    // peer mirror) — live git subprocess, same as before this index
    // existed.
    let (git_dir, rev) = super::mirror_and_head(ctx, repo).await?;
    let commits = gitquarry_core::git::browse::search_log(&git_dir, &rev, query, super::MAX_RESULTS, None)
        .await
        .ok()?;
    Some(commits.into_iter().map(|c| (c.sha, c.subject)).collect())
}
