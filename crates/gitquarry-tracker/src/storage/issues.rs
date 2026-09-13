//! Issue metadata — same `s3db` [`Collection`] convention as
//! `gitquarry_core::storage::pull_requests` (see that module's own doc
//! comment for why: metadata like `RepoConfig`, scoped per-repo-uid at
//! runtime, so `#[derive(Entity)]`'s compile-time-literal collection name
//! can't be used and `Collection::new_owned` is the escape hatch instead).
//!
//! Unlike pull requests, issues never cross the core/federation IPC
//! boundary in this version — no `gitquarry-wire` detour, the types just
//! live here.

use crate::storage::REPOS_PREFIX;
use crate::storage::thread::{append_to_thread, list_thread};
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::Collection;
use s3db::{Backend, Cas};
use s3db_crypto::Cipher;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IssueStatus {
    #[default]
    Open,
    Closed,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct IssueMeta {
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub title: String,
    /// Markdown, same rendering path as a PR description.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub status: IssueStatus,
    #[serde(default)]
    pub author: String,
    /// Usernames, validated against `access::list_all_users` at edit time
    /// (`domain::issue::edit`) — never a `Uid`, same free-text-then-validate
    /// convention every other username field in this codebase follows.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Label names, validated against the repo's own `RepoConfig.labels`
    /// vocabulary at edit time. A name that no longer resolves (its label
    /// was deleted from the repo since) is just absent from the rendered
    /// pills — harmless orphan reference, no cleanup needed, same tolerance
    /// as a deleted user still listed in `Group::owners`.
    #[serde(default)]
    pub labels: Vec<String>,
    /// A milestone number ([`super::milestones::MilestoneMeta::number`]),
    /// validated against `super::milestones::get_milestone` at edit time
    /// (`domain::issue::edit`) — `None` for "no milestone", same optional-
    /// single-value shape a milestone's own not-a-vec design implies (an
    /// issue belongs to at most one). A number that no longer resolves
    /// (its milestone was deleted) reads back as if unset, same orphan-
    /// reference tolerance `labels`'s own doc comment describes.
    #[serde(default)]
    pub milestone: Option<u64>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueComment {
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IssueCounter {
    #[serde(default)]
    next: u64,
}

/// Reserved key in every repo's issues collection, never a valid issue
/// number (numbers start at 1) — same convention as
/// `pull_requests::COUNTER_KEY`.
const COUNTER_KEY: &str = "_counter";

/// Same retry bound as `pull_requests::COUNTER_MAX_RETRIES`.
const COUNTER_MAX_RETRIES: u32 = 8;

pub fn issues_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/issues"))
}

/// Mints the next sequential issue number for this repo — same CAS get/
/// increment/put retry loop as `pull_requests::next_pr_number`, against its
/// own counter (issue numbers and PR numbers are independent sequences,
/// same as GitHub's are not).
pub async fn next_issue_number(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<u64> {
    let collection = issues_collection(uid);
    for attempt in 1..=COUNTER_MAX_RETRIES {
        let (counter, etag) = collection.get::<IssueCounter>(backend, cipher, COUNTER_KEY).await?;
        let number = counter.map_or(1, |c| c.next.max(1));
        let updated = IssueCounter { next: number + 1 };
        match collection
            .put(backend, cipher, COUNTER_KEY, &updated, etag.as_deref())
            .await?
        {
            Cas::Ok => return Ok(number),
            Cas::Conflict if attempt < COUNTER_MAX_RETRIES => {
                crate::storage::cas_retry_backoff(attempt).await;
                continue;
            }
            Cas::Conflict => break,
        }
    }
    bail!("too many CAS conflicts minting an issue number (giving up after {COUNTER_MAX_RETRIES} attempts)")
}

/// Current metadata for `number` and its etag; `None` if no such issue exists.
pub async fn get_issue(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
) -> Result<(Option<IssueMeta>, Option<String>)> {
    issues_collection(uid).get(backend, cipher, &number.to_string()).await
}

/// Compare-and-swap write, keyed by `issue.number` — same
/// create-or-edit dual role as `pull_requests::put_pull_request`.
pub async fn put_issue(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    issue: &IssueMeta,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    issues_collection(uid)
        .put(backend, cipher, &issue.number.to_string(), issue, expected_etag)
        .await
}

/// Every issue currently stored for this repo, newest-number-first. Skips
/// [`COUNTER_KEY`], same reasoning as `pull_requests::list_pull_requests`.
pub async fn list_issues(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>, uid: &Uid) -> Result<Vec<IssueMeta>> {
    let collection = issues_collection(uid);
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
        set.spawn(async move { collection.get::<IssueMeta>(backend.as_ref(), &cipher, &name).await });
    }
    let mut issues = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok((Some(issue), _etag))) = result {
            issues.push(issue);
        }
    }
    issues.sort_unstable_by_key(|issue| std::cmp::Reverse(issue.number));
    Ok(issues)
}

fn issue_comments_collection(uid: &Uid, number: u64) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/issues/{number}/comments"))
}

/// Appends one comment to `number`'s thread — built on the shared
/// append-only thread helper `pull_requests` also uses.
pub async fn add_issue_comment(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
    comment: &IssueComment,
) -> Result<()> {
    let collection = issue_comments_collection(uid, number);
    append_to_thread(&collection, backend, cipher, comment, "issue comment").await
}

/// Every comment on `number`, oldest first (chronological), paired with its
/// storage key — needed to later target one specific comment for
/// [`get_issue_comment`]/[`put_issue_comment`]/[`delete_issue_comment`],
/// same "the thread key is the comment's only id" convention
/// `crate::storage::thread::list_thread`'s own doc comment describes.
pub async fn list_issue_comments(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    number: u64,
) -> Result<Vec<(String, IssueComment)>> {
    list_thread(issue_comments_collection(uid, number), backend, cipher).await
}

pub async fn get_issue_comment(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
    comment_id: &str,
) -> Result<(Option<IssueComment>, Option<String>)> {
    issue_comments_collection(uid, number)
        .get(backend, cipher, comment_id)
        .await
}

pub async fn put_issue_comment(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
    comment_id: &str,
    comment: &IssueComment,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    issue_comments_collection(uid, number)
        .put(backend, cipher, comment_id, comment, expected_etag)
        .await
}

/// Soft-delete (see `s3db::Collection::delete`'s own doc comment) — same
/// best-effort, tombstoned-not-gone convention every other delete in this
/// codebase (a label, a release asset, an ssh key) already uses.
pub async fn delete_issue_comment(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, number: u64, comment_id: &str) {
    let _ = issue_comments_collection(uid, number)
        .delete(backend, cipher, comment_id)
        .await;
}
