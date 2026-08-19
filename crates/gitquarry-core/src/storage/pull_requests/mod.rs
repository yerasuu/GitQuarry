//! Pull request metadata — an `s3db` [`Collection`] scoped per repo uid, not
//! [`super::remote::Remote`]: `CLAUDE.md`'s storage convention reserves
//! `Remote` for git repo data (packs/manifest), LFS objects, and release
//! files; a pull request is metadata like `RepoConfig`, so it follows the
//! `s3db` convention instead. It can't use `#[derive(Entity)]` though —
//! that macro's `#[entity(collection = "...")]` needs a compile-time string
//! literal, and this collection's prefix has to include the *target* repo's
//! uid at runtime (see `RepoConfig`'s own doc comment on this exact
//! limitation) — so it's built directly with `Collection::new_owned`,
//! the same escape hatch `storage::settings::owner_index`/`visible_index`
//! and `auth::access::repo_permission_index` already use for other
//! runtime-prefixed, per-repo-uid collections.
//!
//! A cross-fork PR still lives on the *target* repo, never the fork it
//! proposes merging from — same as a GitHub PR belongs to the upstream repo.

use super::uid::Uid;
use super::{Backend, Cas, REPOS_PREFIX};
use crate::crypto::Cipher;
use anyhow::{Result, bail};
use s3db::Collection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// [`PrStatus`], [`PullRequestMeta`], [`PrComment`], [`PrDetail`] now live
/// in `gitquarry-wire` — they cross the (future) core/federation IPC
/// boundary, so both sides need the identical type. Re-exported here so
/// existing call sites (`storage::pull_requests::PullRequestMeta`, ...)
/// don't need to change.
pub use gitquarry_wire::{
    CommentAnchor, DiffSide, MergeStrategy, PrComment, PrDetail, PrReview, PrStatus, PullRequestMeta, ReviewVerdict,
};

#[derive(Debug, Default, Serialize, Deserialize)]
struct PrCounter {
    #[serde(default)]
    next: u64,
}

/// Reserved key in every repo's pulls collection, never a valid PR number
/// (numbers start at 1) — holds the [`PrCounter`] that mints them.
const COUNTER_KEY: &str = "_counter";

/// How many CAS-conflict retries [`next_pr_number`] attempts before giving
/// up — same bound `Collection::write_object_for_restore` uses internally
/// for its own CAS-with-reread loop.
const COUNTER_MAX_RETRIES: u32 = 8;

pub fn pulls_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/pulls"))
}

/// Mints the next sequential PR number for this repo: a CAS get/increment/
/// put retry loop against [`COUNTER_KEY`]. `s3db` has no counter/sequence
/// primitive of its own — nothing in this codebase needed one before (the
/// audit log's keys are timestamp+random specifically to avoid needing a
/// counter, see `storage::audit`'s own doc comment).
pub async fn next_pr_number(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<u64> {
    let collection = pulls_collection(uid);
    for attempt in 1..=COUNTER_MAX_RETRIES {
        let (counter, etag) = collection.get::<PrCounter>(backend, cipher, COUNTER_KEY).await?;
        let number = counter.map_or(1, |c| c.next.max(1));
        let updated = PrCounter { next: number + 1 };
        match collection
            .put(backend, cipher, COUNTER_KEY, &updated, etag.as_deref())
            .await?
        {
            Cas::Ok => return Ok(number),
            Cas::Conflict if attempt < COUNTER_MAX_RETRIES => {
                super::cas_retry_backoff(attempt).await;
                continue;
            }
            Cas::Conflict => break,
        }
    }
    bail!("too many CAS conflicts minting a PR number (giving up after {COUNTER_MAX_RETRIES} attempts)")
}

/// Current metadata for `number` and its etag; `None` if no such PR exists.
pub async fn get_pull_request(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
) -> Result<(Option<PullRequestMeta>, Option<String>)> {
    pulls_collection(uid).get(backend, cipher, &number.to_string()).await
}

/// Compare-and-swap write, keyed by `pr.number`. `expected_etag = None`
/// both creates (the number came from [`next_pr_number`], so a
/// [`Cas::Conflict`] here would mean a genuine bug, not routine contention)
/// and lets a caller CAS an edit of an existing PR (status transitions).
pub async fn put_pull_request(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    pr: &PullRequestMeta,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    pulls_collection(uid)
        .put(backend, cipher, &pr.number.to_string(), pr, expected_etag)
        .await
}

/// Every PR currently stored for this repo, newest-number-first. Skips
/// [`COUNTER_KEY`] explicitly rather than relying on it failing to parse as
/// a [`PullRequestMeta`] (every field here defaults, so it would parse —
/// just to a bogus all-defaults entry).
pub async fn list_pull_requests(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
) -> Result<Vec<PullRequestMeta>> {
    let collection = pulls_collection(uid);
    let names: Vec<String> = collection
        .list_names(backend.as_ref())
        .await?
        .into_iter()
        .filter(|name| name != COUNTER_KEY)
        .collect();

    let mut set = tokio::task::JoinSet::new();
    for name in names {
        let collection = collection.clone();
        let backend = Arc::clone(backend);
        let cipher = Arc::clone(cipher);
        set.spawn(async move {
            collection
                .get::<PullRequestMeta>(backend.as_ref(), &cipher, &name)
                .await
        });
    }
    let mut prs = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok((Some(pr), _etag))) = result {
            prs.push(pr);
        }
    }
    prs.sort_unstable_by_key(|pr| std::cmp::Reverse(pr.number));
    Ok(prs)
}

fn pr_comments_collection(uid: &Uid, number: u64) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/pulls/{number}/comments"))
}

fn pr_reviews_collection(uid: &Uid, number: u64) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/pulls/{number}/reviews"))
}

/// Appends one review to `number`'s review thread. Reviews are never
/// edited in place — a reviewer changing their mind submits another one,
/// and only their latest counts (`domain::pull_request::review_summary`).
pub async fn add_pr_review(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
    review: &PrReview,
) -> Result<()> {
    let collection = pr_reviews_collection(uid, number);
    super::thread::append_to_thread(&collection, backend, cipher, review, "review").await
}

/// Every review on `number`, oldest first.
pub async fn list_pr_reviews(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    number: u64,
) -> Result<Vec<PrReview>> {
    Ok(
        super::thread::list_thread(pr_reviews_collection(uid, number), backend, cipher)
            .await?
            .into_iter()
            .map(|(_key, review)| review)
            .collect(),
    )
}

/// Appends one comment to `number`'s thread. `Err` only on a genuine
/// backend/serialization failure or the astronomically unlikely nanos+random
/// key collision — never a routine [`Cas::Conflict`], since every key is
/// unique by construction.
pub async fn add_pr_comment(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
    comment: &PrComment,
) -> Result<()> {
    let collection = pr_comments_collection(uid, number);
    super::thread::append_to_thread(&collection, backend, cipher, comment, "comment").await
}

/// Every comment on `number`, oldest first (chronological — see
/// `storage::thread::list_thread`'s own doc comment on why sorted names
/// already are).
pub async fn list_pr_comments(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    number: u64,
) -> Result<Vec<PrComment>> {
    Ok(list_pr_comments_with_keys(backend, cipher, uid, number)
        .await?
        .into_iter()
        .map(|(_key, comment)| comment)
        .collect())
}

/// Same as [`list_pr_comments`], but keeps each comment's own storage key —
/// see `storage::thread::list_thread`'s own doc comment on why Track B's
/// PR sync needs one.
pub async fn list_pr_comments_with_keys(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    number: u64,
) -> Result<Vec<(String, PrComment)>> {
    super::thread::list_thread(pr_comments_collection(uid, number), backend, cipher).await
}

pub async fn get_pr_comment(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
    comment_id: &str,
) -> Result<(Option<PrComment>, Option<String>)> {
    pr_comments_collection(uid, number)
        .get(backend, cipher, comment_id)
        .await
}

pub async fn put_pr_comment(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
    comment_id: &str,
    comment: &PrComment,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    pr_comments_collection(uid, number)
        .put(backend, cipher, comment_id, comment, expected_etag)
        .await
}

/// Soft-delete — same best-effort, tombstoned-not-gone convention as
/// `storage::issues::delete_issue_comment`.
pub async fn delete_pr_comment(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, number: u64, comment_id: &str) {
    pr_comments_collection(uid, number)
        .delete(backend, cipher, comment_id)
        .await
}
