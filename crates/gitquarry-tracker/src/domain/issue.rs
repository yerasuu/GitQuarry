//! Every mutation of a [`crate::storage::issues::IssueMeta`] goes through
//! here — transport-agnostic, `anyhow::Result`, no CSRF/session/audit
//! concerns, those stay in `webui::repo::issues`.
//!
//! Unlike a pull request, an issue has no merge/diff/shadow-ref machinery —
//! it's just a title, a markdown body, a status, labels, and assignees.

use crate::i18n;
use crate::state::{TrackerState, TrackerTarget};
use crate::storage::issues::{self, IssueComment, IssueMeta, IssueStatus};
use crate::storage::milestones;
use anyhow::{Result, bail};
use gitquarry_i18n::Locale;
use s3db::Cas;
use std::time::{SystemTime, UNIX_EPOCH};

/// Duplicated from `gitquarry_core::storage::audit::format_unix_secs` — same
/// "cheap to duplicate a few lines" reasoning as `storage::thread`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn format_unix_secs(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_secs(secs)
}

/// Duplicated from `gitquarry_core::validate::validate_issue_title` — a
/// small, pure text check, not worth a shared crate for it.
const MAX_ISSUE_TITLE_LEN: usize = 256;

fn validate_issue_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.len() > MAX_ISSUE_TITLE_LEN {
        bail!("empty or too-long issue title (max {MAX_ISSUE_TITLE_LEN} bytes)");
    }
    if title.contains(|c: char| c.is_control()) {
        bail!("invalid issue title (control characters)");
    }
    Ok(title.to_string())
}

/// Duplicated from `gitquarry_core::domain::pull_request::closing_issue_numbers`
/// — a small, pure text-parsing function, no I/O, not worth a shared crate
/// (same reasoning `gitquarry-graph-validate`'s design explicitly *rejected*
/// for the DFS cycle check, which is non-trivial enough to justify a crate —
/// this doesn't clear that bar).
const CLOSING_KEYWORDS: &[&str] = &[
    "closes", "closed", "close", "fixes", "fixed", "fix", "resolves", "resolved", "resolve",
];

fn closing_issue_numbers(text: &str) -> Vec<u64> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut numbers = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if starts_word && let Some(keyword) = CLOSING_KEYWORDS.iter().find(|k| lower[i..].starts_with(**k)) {
            let after_keyword = i + keyword.len();
            if bytes.get(after_keyword).is_none_or(|b| !b.is_ascii_alphanumeric()) {
                let mut j = after_keyword;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b':') {
                    j += 1;
                }
                if bytes.get(j) == Some(&b'#') {
                    let mut k = j + 1;
                    while k < bytes.len() && bytes[k].is_ascii_digit() {
                        k += 1;
                    }
                    if k > j + 1 {
                        if let Ok(number) = lower[j + 1..k].parse::<u64>() {
                            numbers.push(number);
                        }
                        i = k;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

pub async fn create(
    state: &TrackerState,
    target: &TrackerTarget,
    title: &str,
    body: &str,
    author: &str,
) -> Result<IssueMeta> {
    let title = validate_issue_title(title)?;
    let number = issues::next_issue_number(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let stamp = now_stamp();
    let issue = IssueMeta {
        number,
        title,
        body: body.to_string(),
        status: IssueStatus::Open,
        author: author.to_string(),
        assignees: Vec::new(),
        labels: Vec::new(),
        milestone: None,
        created_at: stamp.clone(),
        updated_at: stamp,
    };
    let cas = issues::put_issue(state.backend.as_ref(), &state.cipher, &target.uid, &issue, None).await?;
    if cas != Cas::Ok {
        bail!("issue number collision — the counter is supposed to guarantee this never happens");
    }
    state
        .core
        .push_event(&gitquarry_wire::tracker::TrackerEvent {
            repo_uid: target.uid.clone(),
            kind: gitquarry_wire::tracker::TrackerEventKind::IssueOpened { number: issue.number },
        })
        .await;
    Ok(issue)
}

pub async fn get(state: &TrackerState, target: &TrackerTarget, number: u64) -> Result<Option<IssueMeta>> {
    let (issue, _etag) = issues::get_issue(state.backend.as_ref(), &state.cipher, &target.uid, number).await?;
    Ok(issue)
}

/// Every issue on `target`, newest-first, optionally narrowed to one status.
pub async fn list(
    state: &TrackerState,
    target: &TrackerTarget,
    status_filter: Option<IssueStatus>,
) -> Result<Vec<IssueMeta>> {
    let mut issues = issues::list_issues(&state.backend, &state.cipher, &target.uid).await?;
    if let Some(status) = status_filter {
        issues.retain(|issue| issue.status == status);
    }
    Ok(issues)
}

/// Bundled so [`edit`] doesn't cross clippy's `too_many_arguments`.
/// `labels_csv`/`assignees_csv` are comma-separated free text.
pub struct IssueEdit<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub labels_csv: &'a str,
    pub assignees_csv: &'a str,
    /// `None` clears the milestone; `Some(number)` must resolve to a real
    /// milestone on `target`.
    pub milestone: Option<u64>,
}

/// One combined edit: title, body, labels, and assignees all change
/// together in a single get-etag/mutate/CAS-put. Every label in
/// `edit.labels_csv` must already exist in `target`'s label vocabulary
/// (checked via `state.core.repo_labels`, no longer a direct `RepoConfig`
/// read now that tracker is out-of-process), every assignee in
/// `edit.assignees_csv` must resolve to a real user (`state.core.user_exists`,
/// no longer a direct `access::get_user` call) — the first one that doesn't
/// bails with a clear error naming it, before anything is written.
pub async fn edit(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    edit: IssueEdit<'_>,
    locale: Locale,
) -> Result<Cas> {
    let title = validate_issue_title(edit.title)?;

    let requested_labels: Vec<&str> = edit
        .labels_csv
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    let mut labels = Vec::new();
    if !requested_labels.is_empty() {
        // Only fetched when there's actually something to validate against
        // — unlike the in-process `target.remote.get_repo_config()` read
        // this replaced, this is a real network round trip now.
        let known_labels = state.core.repo_labels(target.uid.as_str()).await?;
        let known_labels: std::collections::BTreeSet<&str> = known_labels.iter().map(String::as_str).collect();
        for name in requested_labels {
            if !known_labels.contains(name) {
                bail!("{}", i18n::unknown_label(locale, name));
            }
            labels.push(name.to_string());
        }
    }

    let mut assignees = Vec::new();
    for username in edit.assignees_csv.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if !state.core.user_exists(username).await? {
            bail!("{}", i18n::unknown_assignee(locale, username));
        }
        assignees.push(username.to_string());
    }

    if let Some(milestone_number) = edit.milestone {
        let (milestone, _etag) =
            milestones::get_milestone(state.backend.as_ref(), &state.cipher, &target.uid, milestone_number).await?;
        if milestone.is_none() {
            bail!("{}", i18n::milestone_not_found(locale, milestone_number));
        }
    }

    let (issue, etag) = issues::get_issue(state.backend.as_ref(), &state.cipher, &target.uid, number).await?;
    let Some(mut issue) = issue else {
        bail!("{}", i18n::issue_not_found(locale, number));
    };
    let old_milestone = issue.milestone;
    let status = issue.status;
    issue.title = title;
    issue.body = edit.body.to_string();
    issue.labels = labels;
    issue.assignees = assignees;
    issue.milestone = edit.milestone;
    issue.updated_at = now_stamp();
    let cas = issues::put_issue(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &issue,
        etag.as_deref(),
    )
    .await?;
    // Only once the reassignment itself actually landed — a `Cas::Conflict`
    // means this edit never happened, so the milestone counts must stay
    // exactly as they were.
    if cas == Cas::Ok && old_milestone != edit.milestone {
        let (open_delta, closed_delta) = match status {
            IssueStatus::Open => (-1, 0),
            IssueStatus::Closed => (0, -1),
        };
        if let Some(old) = old_milestone {
            crate::domain::milestone::bump_counts(state, target, old, open_delta, closed_delta).await;
        }
        if let Some(new) = edit.milestone {
            crate::domain::milestone::bump_counts(state, target, new, -open_delta, -closed_delta).await;
        }
    }
    Ok(cas)
}

/// `Err` if `number` doesn't exist or isn't currently in `expected` status —
/// no reopening an already-open issue, no closing an already-closed one.
async fn transition(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    expected: IssueStatus,
    next: IssueStatus,
    locale: Locale,
) -> Result<Cas> {
    let (issue, etag) = issues::get_issue(state.backend.as_ref(), &state.cipher, &target.uid, number).await?;
    let Some(mut issue) = issue else {
        bail!("{}", i18n::issue_not_found(locale, number));
    };
    if issue.status != expected {
        bail!("{}", i18n::issue_wrong_status(locale));
    }
    let milestone = issue.milestone;
    issue.status = next;
    issue.updated_at = now_stamp();
    let cas = issues::put_issue(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &issue,
        etag.as_deref(),
    )
    .await?;
    if cas == Cas::Ok
        && let Some(number) = milestone
    {
        let (open_delta, closed_delta) = match next {
            IssueStatus::Closed => (-1, 1),
            IssueStatus::Open => (1, -1),
        };
        crate::domain::milestone::bump_counts(state, target, number, open_delta, closed_delta).await;
    }
    Ok(cas)
}

pub async fn close(state: &TrackerState, target: &TrackerTarget, number: u64, locale: Locale) -> Result<Cas> {
    let cas = transition(state, target, number, IssueStatus::Open, IssueStatus::Closed, locale).await?;
    if cas == Cas::Ok {
        state
            .core
            .push_event(&gitquarry_wire::tracker::TrackerEvent {
                repo_uid: target.uid.clone(),
                kind: gitquarry_wire::tracker::TrackerEventKind::IssueClosed { number },
            })
            .await;
    }
    Ok(cas)
}

pub async fn reopen(state: &TrackerState, target: &TrackerTarget, number: u64, locale: Locale) -> Result<Cas> {
    transition(state, target, number, IssueStatus::Closed, IssueStatus::Open, locale).await
}

/// The `POST /_internal/tracker/pull-request-merged` handler's own logic —
/// what used to be a `RepoEvent::PullRequest{action: Merged}` in-process bus
/// consumer before the binary split, now a pushed notification instead
/// (core's push already carries `title`/`description`, no round trip back
/// needed to fetch them). Best-effort per number, same tolerance the
/// in-process version already had: a nonexistent or already-closed issue
/// number is silently skipped, since the merge that triggered this already
/// succeeded.
pub async fn close_referenced_by_pull_request(
    state: &TrackerState,
    target: &TrackerTarget,
    title: &str,
    description: &str,
    locale: Locale,
) -> Result<()> {
    for issue_number in closing_issue_numbers(&format!("{title} {description}")) {
        let _ = close(state, target, issue_number, locale).await;
    }
    Ok(())
}

/// `Err` if `number` doesn't exist — a comment can't be left on an issue
/// that was never created.
pub async fn add_comment(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    author: &str,
    body: &str,
    locale: Locale,
) -> Result<()> {
    if get(state, target, number).await?.is_none() {
        bail!("{}", i18n::issue_not_found(locale, number));
    }
    let comment = IssueComment {
        author: author.to_string(),
        body: body.to_string(),
        created_at: now_stamp(),
    };
    issues::add_issue_comment(state.backend.as_ref(), &state.cipher, &target.uid, number, &comment).await
}

/// Oldest first, each paired with the storage key `edit_comment`/
/// `delete_comment` need to target it.
pub async fn list_comments(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
) -> Result<Vec<(String, IssueComment)>> {
    issues::list_issue_comments(&state.backend, &state.cipher, &target.uid, number).await
}

/// `Err` if `comment_id` doesn't exist or `actor` isn't the comment's own
/// author — author-only, no `Developer`+ override.
pub async fn edit_comment(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    comment_id: &str,
    actor: &str,
    body: &str,
    locale: Locale,
) -> Result<Cas> {
    let (comment, etag) =
        issues::get_issue_comment(state.backend.as_ref(), &state.cipher, &target.uid, number, comment_id).await?;
    let Some(mut comment) = comment else {
        bail!("comment {comment_id} not found on issue #{number}");
    };
    if comment.author != actor {
        bail!("{}", i18n::not_comment_author(locale));
    }
    comment.body = body.to_string();
    issues::put_issue_comment(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        number,
        comment_id,
        &comment,
        etag.as_deref(),
    )
    .await
}

/// Same author-only bar as [`edit_comment`] — *unless* `is_moderator` (the
/// caller's own permission check, resolved in `webui` before this is ever
/// called). Deliberately delete-only: [`edit_comment`] stays strictly
/// author-only, no override.
pub async fn delete_comment(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    comment_id: &str,
    actor: &str,
    is_moderator: bool,
    locale: Locale,
) -> Result<()> {
    let (comment, _etag) =
        issues::get_issue_comment(state.backend.as_ref(), &state.cipher, &target.uid, number, comment_id).await?;
    let Some(comment) = comment else {
        bail!("comment {comment_id} not found on issue #{number}");
    };
    if comment.author != actor && !is_moderator {
        bail!("{}", i18n::not_comment_author(locale));
    }
    issues::delete_issue_comment(state.backend.as_ref(), &state.cipher, &target.uid, number, comment_id).await;
    Ok(())
}
