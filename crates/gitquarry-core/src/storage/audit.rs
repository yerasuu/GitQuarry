//! Append-only trail of admin/repo actions, bucket-side under `.conf/audit/`
//! — sibling of [`super::settings`]'s `.conf/global.json`, same reserved
//! folder. Unlike every other bucket-side config this is never
//! read-modify-written: each event gets its own key (timestamp + random
//! suffix) inside a per-month [`Collection`] (see [`audit_collection`]), so
//! concurrent writers never contend over one CAS'd file the way a
//! `User`/`Group`/`RepoConfig` writer does — `Collection::put`'s
//! create-only semantics (`expected_etag: None`) already assume exactly
//! that shape.

use super::{Backend, Cas, get_json, list_all_keys};
use crate::crypto::Cipher;
use anyhow::Result;
use s3db::Collection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// Collection-prefix root (no trailing slash — [`Collection::new_owned`]
/// adds the `/` itself). One [`Collection`] per month bucket, built at
/// [`audit_collection`] — see [`month_bucket`] for why entries are split
/// this way instead of one flat collection.
const AUDIT_ROOT: &str = ".conf/audit";
/// Same value as [`AUDIT_ROOT`], public because the backup retention pass
/// (`crate::backup::prune`) deletes whole month prefixes under it at a
/// *destination* bucket, and hardcoding the string in a second place is how
/// the two would eventually disagree.
pub const AUDIT_ROOT_PUBLIC: &str = AUDIT_ROOT;
#[cfg(any(test, feature = "test-util"))]
pub const AUDIT_PREFIX: &str = ".conf/audit/";
#[cfg(not(any(test, feature = "test-util")))]
const AUDIT_PREFIX: &str = ".conf/audit/";

/// The [`Collection`] holding one month's worth of audit entries — built
/// per call rather than cached, same reasoning as any other
/// runtime-scoped [`Collection::new_owned`] (e.g. a repo's permission
/// collection): cheap (two small owned fields), and the month it should
/// point at is only known at the call site.
#[cfg(any(test, feature = "test-util"))]
pub fn audit_collection(month: &str) -> Collection {
    audit_collection_impl(month)
}
#[cfg(not(any(test, feature = "test-util")))]
fn audit_collection(month: &str) -> Collection {
    audit_collection_impl(month)
}
fn audit_collection_impl(month: &str) -> Collection {
    Collection::new_owned(format!("{AUDIT_ROOT}/{month}"))
}

/// How many entries `/_audit` shows — a recent-activity feed, not a
/// paginated log viewer.
pub const LIST_LIMIT: usize = 200;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditEntry {
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub actor: String,
    /// Short dotted slug, e.g. `"repo.delete"`, `"group.add_owner"` — not
    /// free text, so `/_audit` can render it as-is without escaping worries
    /// beyond the usual HTML-escape.
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// `"YYYY-MM"` for the month `secs` (Unix seconds) falls in — the first key
/// segment after [`AUDIT_PREFIX`], so a whole month's entries share one
/// prefix and [`recent_keys`] can LIST just the last handful of months
/// instead of the audit log's entire lifetime.
#[cfg(any(test, feature = "test-util"))]
pub fn month_bucket(secs: u64) -> String {
    month_bucket_impl(secs)
}
#[cfg(not(any(test, feature = "test-util")))]
fn month_bucket(secs: u64) -> String {
    month_bucket_impl(secs)
}
fn month_bucket_impl(secs: u64) -> String {
    let (y, mo, _d) = civil_from_days((secs / 86400) as i64);
    format!("{y:04}-{mo:02}")
}

#[cfg(any(test, feature = "test-util"))]
pub fn entry_name(at_millis: u128) -> String {
    entry_name_impl(at_millis)
}
#[cfg(not(any(test, feature = "test-util")))]
fn entry_name(at_millis: u128) -> String {
    entry_name_impl(at_millis)
}
/// Name (not full key — [`audit_collection`] owns the `{AUDIT_ROOT}/{month}`
/// prefix and `.json` suffix) for one entry within its month bucket.
/// Zero-padded millis first: lexicographic order == chronological order,
/// which is what makes "list, sort names, take the tail" work without
/// reading every entry's body first.
fn entry_name_impl(at_millis: u128) -> String {
    format!("{at_millis:020}-{:08x}", rand::random::<u32>())
}

/// One step back from `(year, month)` — `month` is 1-12, wraps `year` on a
/// January underflow. `day` rides along unused (civil_from_days always
/// returns one) so callers don't need to destructure/rebuild the tuple.
#[cfg(any(test, feature = "test-util"))]
pub fn prev_month(ymd: (i64, u32, u32)) -> (i64, u32, u32) {
    prev_month_impl(ymd)
}
#[cfg(not(any(test, feature = "test-util")))]
fn prev_month(ymd: (i64, u32, u32)) -> (i64, u32, u32) {
    prev_month_impl(ymd)
}
fn prev_month_impl((y, mo, d): (i64, u32, u32)) -> (i64, u32, u32) {
    if mo <= 1 { (y - 1, 12, d) } else { (y, mo - 1, d) }
}

/// How many months [`recent_keys`] walks backward before giving up on
/// finding [`LIST_LIMIT`] entries via month buckets and falling back to a
/// full scan. High enough that any bucket with at least a little monthly
/// audit activity never hits the fallback; low enough to still bound the
/// worst case to a year of month-prefix LISTs, not the whole history.
const MAX_MONTHS_BACK: u32 = 12;

/// Up to [`LIST_LIMIT`] of the most recent audit keys, without listing the
/// whole `.conf/audit/` prefix on a bucket with deep history: walks
/// backward month by month (see [`month_bucket`]) accumulating keys until
/// there are enough, sorts, and truncates. A bucket whose last
/// [`MAX_MONTHS_BACK`] months don't add up to [`LIST_LIMIT`] entries falls
/// through to one full-prefix LIST — the audit log evidently doesn't have
/// much in it either way, so that's cheap exactly when it triggers. The
/// full-prefix fallback also matches legacy entries written before month
/// buckets existed (flat `.conf/audit/<millis>-<rand>.json`, no `/YYYY-MM/`
/// segment) still under the same `AUDIT_PREFIX`, so nothing written before
/// this change goes missing.
async fn recent_keys(backend: &dyn Backend) -> Result<Vec<String>> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut cursor = civil_from_days((now / 86400) as i64);
    let mut collected = Vec::new();
    for _ in 0..MAX_MONTHS_BACK {
        let (y, mo, _d) = cursor;
        let coll = audit_collection(&format!("{y:04}-{mo:02}"));
        for name in coll.list_names(backend).await? {
            collected.push(coll.key(&name)?);
        }
        if collected.len() >= LIST_LIMIT {
            collected.sort_unstable_by(|a, b| b.cmp(a));
            collected.truncate(LIST_LIMIT);
            return Ok(collected);
        }
        cursor = prev_month(cursor);
    }
    let mut all = list_all_keys(backend, AUDIT_PREFIX).await?;
    all.sort_unstable_by(|a, b| b.cmp(a));
    all.truncate(LIST_LIMIT);
    Ok(all)
}

/// Records one event. Best-effort: a failure here (network blip, the
/// once-in-an-eon random-suffix collision) must never fail or roll back the
/// action it's describing — it just doesn't get logged and the caller
/// never finds out.
pub async fn record(
    backend: &dyn Backend,
    cipher: &Cipher,
    actor: &str,
    action: &str,
    target: &str,
    detail: Option<String>,
) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let entry = AuditEntry {
        at: format_unix_secs(now.as_secs()),
        actor: actor.to_string(),
        action: action.to_string(),
        target: target.to_string(),
        detail,
    };
    let month = month_bucket(now.as_secs());
    let name = entry_name(now.as_millis());
    let coll = audit_collection(&month);
    match coll.put(backend, cipher, &name, &entry, None).await {
        Ok(Cas::Ok) => {}
        Ok(Cas::Conflict) => {
            warn!(key = %coll.key(&name).unwrap_or(name), "key collision in audit log, entry lost")
        }
        Err(e) => warn!(error = %e, "couldn't write audit log"),
    }
}

/// Most recent [`LIST_LIMIT`] entries, newest first. Candidate keys come
/// from [`recent_keys`] (a bounded, month-at-a-time LIST — see its own doc
/// comment), already in the right order (its own key format sorts
/// lexicographically by millisecond, then a random tie-breaker); only the
/// bodies of the ones actually shown get fetched, concurrently, then
/// reassembled back into that same order by index rather than re-sorted by
/// `AuditEntry::at` — `at` is only second-precision (see its own doc
/// comment), so two entries recorded within the same second (routine: any
/// two admin actions a moment apart) would otherwise tie and fall back to
/// whatever order the concurrent fetch happened to complete in. Takes an
/// owned `Arc<dyn Backend>` (not `&dyn Backend` like every other storage
/// function) because the per-key fetches below run concurrently in their
/// own spawned tasks, each needing its own `'static` clone.
pub async fn list_recent(backend: &Arc<dyn Backend>, cipher: &Cipher) -> Result<Vec<AuditEntry>> {
    let keys = recent_keys(backend.as_ref()).await?;

    let mut set = tokio::task::JoinSet::new();
    for (index, key) in keys.into_iter().enumerate() {
        let backend = Arc::clone(backend);
        let cipher = cipher.clone();
        set.spawn(async move { (index, get_json::<AuditEntry>(backend.as_ref(), &key, &cipher).await) });
    }
    let mut entries = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok((index, Ok((entry, _etag)))) = result {
            entries.push((index, entry));
        }
    }
    entries.sort_unstable_by_key(|(index, _)| *index);
    Ok(entries.into_iter().map(|(_, entry)| entry).collect())
}

/// [`list_recent`]'s entries filtered down to one `target` repo — what a
/// repo's own "activity" view shows alongside its commits. Filters after
/// the fact rather than listing `.conf/audit/` a second time with a
/// different prefix: entries aren't keyed by target, only by timestamp, so
/// there's no cheaper way to scope the same recent window to one repo.
pub async fn list_recent_for(backend: &Arc<dyn Backend>, cipher: &Cipher, target: &str) -> Result<Vec<AuditEntry>> {
    let entries = list_recent(backend, cipher).await?;
    Ok(entries.into_iter().filter(|e| e.target == target).collect())
}

/// `secs` since the Unix epoch -> `"YYYY-MM-DDTHH:MM:SSZ"`. No calendar
/// crate in this project's dependency tree just for this one label, so this
/// is Howard Hinnant's `civil_from_days` (public-domain, the same algorithm
/// most date libraries use under the hood), proleptic Gregorian. `pub`, not
/// `pub(crate)`: other bucket-side timestamp fields in this crate
/// (`storage::remote::ReleaseMeta`/`AssetMeta`) reuse it, and so does the
/// `gitquarry-tracker` crate's own issue/milestone timestamps
/// (`docs/tracker-extension-concept.md`) — a separate crate needs full
/// `pub`, `pub(crate)` isn't visible across the crate boundary.
pub fn format_unix_secs(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

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
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
