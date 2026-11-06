//! Every mutation of a [`crate::storage::milestones::MilestoneMeta`] goes
//! through here — same `domain`/`web` boundary [`crate::domain::issue`]'s
//! doc comment describes: transport-agnostic, `anyhow::Result`, no CSRF/
//! session/audit concerns, those stay in `webui::repo::milestones`.
//!
//! A milestone has no comments/assignees of its own — just a title, an
//! optional description and due date, and a status. An issue points at a
//! milestone by number ([`crate::storage::issues::IssueMeta::milestone`]).
//! Unlike a label or an assignee, a milestone *does* carry a back-reference:
//! [`MilestoneMeta::open_issues`]/`closed_issues`, maintained incrementally
//! by [`bump_counts`] from `domain::issue::edit` (milestone reassignment)
//! and `domain::issue::transition` (status change).

use crate::i18n;
use crate::state::{TrackerState, TrackerTarget};
use crate::storage::milestones::{self, MilestoneMeta, MilestoneStatus};
use anyhow::{Result, bail};
use gitquarry_i18n::Locale;
use s3db::Cas;
use std::time::{SystemTime, UNIX_EPOCH};

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

pub async fn create(
    state: &TrackerState,
    target: &TrackerTarget,
    title: &str,
    description: &str,
    due_date: &str,
) -> Result<MilestoneMeta> {
    let title = title.trim();
    if title.is_empty() {
        bail!("milestone title can't be empty");
    }
    let number = milestones::next_milestone_number(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let stamp = now_stamp();
    let milestone = MilestoneMeta {
        number,
        title: title.to_string(),
        description: description.to_string(),
        due_date: due_date.trim().to_string(),
        status: MilestoneStatus::Open,
        open_issues: 0,
        closed_issues: 0,
        created_at: stamp.clone(),
        updated_at: stamp,
    };
    let cas = milestones::put_milestone(state.backend.as_ref(), &state.cipher, &target.uid, &milestone, None).await?;
    if cas != Cas::Ok {
        bail!("milestone number collision — the counter is supposed to guarantee this never happens");
    }
    Ok(milestone)
}

pub async fn get(state: &TrackerState, target: &TrackerTarget, number: u64) -> Result<Option<MilestoneMeta>> {
    let (milestone, _etag) =
        milestones::get_milestone(state.backend.as_ref(), &state.cipher, &target.uid, number).await?;
    Ok(milestone)
}

/// Title/description/due-date, changed together in a single get-etag/
/// mutate/CAS-put. `status` doesn't move through here; [`close`]/[`reopen`]
/// stay the only way to change it.
pub async fn edit(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    title: &str,
    description: &str,
    due_date: &str,
    locale: Locale,
) -> Result<Cas> {
    let title = title.trim();
    if title.is_empty() {
        bail!("milestone title can't be empty");
    }
    let (milestone, etag) =
        milestones::get_milestone(state.backend.as_ref(), &state.cipher, &target.uid, number).await?;
    let Some(mut milestone) = milestone else {
        bail!("{}", i18n::milestone_not_found(locale, number));
    };
    milestone.title = title.to_string();
    milestone.description = description.to_string();
    milestone.due_date = due_date.trim().to_string();
    milestone.updated_at = now_stamp();
    milestones::put_milestone(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &milestone,
        etag.as_deref(),
    )
    .await
}

/// Every milestone on `target`, newest-first — open and closed both.
pub async fn list(state: &TrackerState, target: &TrackerTarget) -> Result<Vec<MilestoneMeta>> {
    milestones::list_milestones(&state.backend, &state.cipher, &target.uid).await
}

/// `Err` if `number` doesn't exist or isn't currently in `expected` status —
/// same shape as `domain::issue::transition`.
async fn transition(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    expected: MilestoneStatus,
    next: MilestoneStatus,
    locale: Locale,
) -> Result<Cas> {
    let (milestone, etag) =
        milestones::get_milestone(state.backend.as_ref(), &state.cipher, &target.uid, number).await?;
    let Some(mut milestone) = milestone else {
        bail!("{}", i18n::milestone_not_found(locale, number));
    };
    if milestone.status != expected {
        bail!("{}", i18n::milestone_wrong_status(locale));
    }
    milestone.status = next;
    milestone.updated_at = now_stamp();
    milestones::put_milestone(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &milestone,
        etag.as_deref(),
    )
    .await
}

pub async fn close(state: &TrackerState, target: &TrackerTarget, number: u64, locale: Locale) -> Result<Cas> {
    transition(
        state,
        target,
        number,
        MilestoneStatus::Open,
        MilestoneStatus::Closed,
        locale,
    )
    .await
}

pub async fn reopen(state: &TrackerState, target: &TrackerTarget, number: u64, locale: Locale) -> Result<Cas> {
    transition(
        state,
        target,
        number,
        MilestoneStatus::Closed,
        MilestoneStatus::Open,
        locale,
    )
    .await
}

/// Best-effort read/modify/CAS-write against `number`'s own
/// `open_issues`/`closed_issues`. Deltas are signed so one call can express
/// a move between buckets, clamped at zero. A no-op, not an error, if
/// `number` doesn't resolve.
pub async fn bump_counts(
    state: &TrackerState,
    target: &TrackerTarget,
    number: u64,
    open_delta: i32,
    closed_delta: i32,
) {
    let Ok((Some(mut milestone), etag)) =
        milestones::get_milestone(state.backend.as_ref(), &state.cipher, &target.uid, number).await
    else {
        return;
    };
    milestone.open_issues = (i64::from(milestone.open_issues) + i64::from(open_delta)).max(0) as u32;
    milestone.closed_issues = (i64::from(milestone.closed_issues) + i64::from(closed_delta)).max(0) as u32;
    let _ = milestones::put_milestone(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &milestone,
        etag.as_deref(),
    )
    .await;
}
