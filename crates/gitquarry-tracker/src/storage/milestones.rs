//! Milestone metadata — same `s3db` [`Collection`] convention as
//! [`super::issues`] (see that module's own doc comment for why:
//! per-repo-uid-scoped metadata built with `Collection::new_owned` rather
//! than `#[derive(Entity)]`, which needs a compile-time-literal collection
//! name).
//!
//! Like issues, milestones never cross the core/federation IPC boundary in
//! this version — no `gitquarry-wire` detour, the types just live here.

use crate::storage::REPOS_PREFIX;
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::Collection;
use s3db::{Backend, Cas};
use s3db_crypto::Cipher;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MilestoneStatus {
    #[default]
    Open,
    Closed,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MilestoneMeta {
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub title: String,
    /// Markdown, same rendering path as an issue's body.
    #[serde(default)]
    pub description: String,
    /// Free text, not a parsed date — same "store and display, don't
    /// validate the format" tolerance a release's own notes field has.
    /// Empty means no due date set.
    #[serde(default)]
    pub due_date: String,
    #[serde(default)]
    pub status: MilestoneStatus,
    /// How many currently-open issues have this milestone set — maintained
    /// incrementally (`domain::milestone::bump_counts`, called from
    /// `domain::issue::edit`/`transition`) rather than computed by scanning
    /// every issue in the repo on every milestones-page render. Same
    /// "doesn't need to stay in sync with reality" tolerance as
    /// `RepoConfig::fork_count`: a lost race under concurrent issue edits
    /// just drifts the count by one, self-heals on the next successful
    /// milestone (re)assignment or status change.
    #[serde(default)]
    pub open_issues: u32,
    /// Same tolerance as [`Self::open_issues`], for closed ones.
    #[serde(default)]
    pub closed_issues: u32,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MilestoneCounter {
    #[serde(default)]
    next: u64,
}

/// Reserved key in every repo's milestones collection, never a valid
/// milestone number (numbers start at 1) — same convention as
/// `issues::COUNTER_KEY`.
const COUNTER_KEY: &str = "_counter";

/// Same retry bound as `issues::COUNTER_MAX_RETRIES`.
const COUNTER_MAX_RETRIES: u32 = 8;

pub fn milestones_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/milestones"))
}

/// Mints the next sequential milestone number for this repo — same CAS
/// get/increment/put retry loop as `issues::next_issue_number`, against its
/// own counter (milestone numbers, issue numbers, and PR numbers are all
/// independent sequences).
pub async fn next_milestone_number(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<u64> {
    let collection = milestones_collection(uid);
    for attempt in 1..=COUNTER_MAX_RETRIES {
        let (counter, etag) = collection.get::<MilestoneCounter>(backend, cipher, COUNTER_KEY).await?;
        let number = counter.map_or(1, |c| c.next.max(1));
        let updated = MilestoneCounter { next: number + 1 };
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
    bail!("too many CAS conflicts minting a milestone number (giving up after {COUNTER_MAX_RETRIES} attempts)")
}

/// Current metadata for `number` and its etag; `None` if no such milestone
/// exists.
pub async fn get_milestone(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    number: u64,
) -> Result<(Option<MilestoneMeta>, Option<String>)> {
    milestones_collection(uid)
        .get(backend, cipher, &number.to_string())
        .await
}

/// Compare-and-swap write, keyed by `milestone.number` — same
/// create-or-edit dual role as `issues::put_issue`.
pub async fn put_milestone(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    milestone: &MilestoneMeta,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    milestones_collection(uid)
        .put(backend, cipher, &milestone.number.to_string(), milestone, expected_etag)
        .await
}

/// Every milestone currently stored for this repo, newest-number-first.
/// Skips [`COUNTER_KEY`], same reasoning as `issues::list_issues`.
pub async fn list_milestones(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
) -> Result<Vec<MilestoneMeta>> {
    let collection = milestones_collection(uid);
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
        set.spawn(async move { collection.get::<MilestoneMeta>(backend.as_ref(), &cipher, &name).await });
    }
    let mut milestones = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok((Some(milestone), _etag))) = result {
            milestones.push(milestone);
        }
    }
    milestones.sort_unstable_by_key(|milestone| std::cmp::Reverse(milestone.number));
    Ok(milestones)
}
