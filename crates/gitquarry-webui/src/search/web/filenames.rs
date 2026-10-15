//! Filename half of `/_search` — reads each local repo's own stored
//! `gitquarry_core::storage::search_index::SearchIndex::files` (see that module's
//! own doc comment) instead of running `git ls-tree` live; falls back to
//! the live [`gitquarry_core::git::browse::search_filenames`] path for a repo with no
//! index yet (not pushed to since this feature shipped) or a peer mirror
//! (`super::repo_uid_if_local` returns `None` for those — no local uid to
//! key an index on).

use gitquarry_core::app::AppContext;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::task::JoinSet;

pub(super) struct FilenameHit {
    pub repo: String,
    pub rev: String,
    pub path: String,
}

/// `(rev, paths)` hits keyed by repo name, as [`search_all`] accumulates
/// them before flattening.
type HitsByRepo = BTreeMap<String, (String, Vec<String>)>;

/// One finished per-repo scan, as `JoinSet::join_next` yields it.
type ScanResult = Result<(String, Option<(String, Vec<String>)>), tokio::task::JoinError>;

/// Folds one finished scan into the accumulator — same fail-open-per-repo
/// behavior [`search_all`] documents.
fn collect(result: ScanResult, by_repo: &mut HitsByRepo) {
    if let Ok((repo, Some((rev, paths)))) = result
        && !paths.is_empty()
    {
        by_repo.insert(repo, (rev, paths));
    }
}

/// Every filename match for `query` across `repo_names`, sorted by repo
/// name for a deterministic page, capped at [`super::MAX_RESULTS`] total —
/// the `bool` is whether the cap actually cut something off. A repo with no
/// mirror yet, no commits, or a transient fetch error just contributes no
/// hits, same as [`super::commits::search_all`].
pub(super) async fn search_all(ctx: &Arc<AppContext>, repo_names: &[String], query: &str) -> (Vec<FilenameHit>, bool) {
    let mut set = JoinSet::new();
    let mut by_repo: HitsByRepo = BTreeMap::new();
    for repo in repo_names {
        // Drain one before starting the next once the window is full — see
        // `super::MAX_CONCURRENT_REPO_SCANS`, same bound the commit half
        // applies.
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
    let mut all: Vec<FilenameHit> = by_repo
        .into_iter()
        .flat_map(|(repo, (rev, paths))| {
            paths.into_iter().map(move |path| FilenameHit {
                repo: repo.clone(),
                rev: rev.clone(),
                path,
            })
        })
        .collect();
    let truncated = all.len() > super::MAX_RESULTS;
    all.truncate(super::MAX_RESULTS);
    (all, truncated)
}

async fn search_one(ctx: &AppContext, repo: &str, query: &str) -> Option<(String, Vec<String>)> {
    if let Some(uid) = super::repo_uid_if_local(ctx, repo).await
        && let Ok((Some(index), _etag)) =
            gitquarry_core::storage::search_index::get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid).await
        && let Some(head) = index.last_indexed_sha
    {
        let query_lower = query.to_ascii_lowercase();
        let matches = index
            .files
            .into_iter()
            .filter(|f| f.to_ascii_lowercase().contains(&query_lower))
            .collect();
        return Some((head, matches));
    }
    // No stored index (never pushed to since this feature shipped, or a
    // peer mirror) — live git subprocess, same as before this index
    // existed.
    let (git_dir, rev) = super::mirror_and_head(ctx, repo).await?;
    let matches = gitquarry_core::git::browse::search_filenames(&git_dir, &rev, query)
        .await
        .ok()?;
    Some((rev, matches))
}
