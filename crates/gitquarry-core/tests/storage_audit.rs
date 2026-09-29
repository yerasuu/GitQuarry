use gitquarry_core::storage::audit::*;
use gitquarry_test_support::{backend, cipher};
use s3db::Collection;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn epoch_formats_as_1970() {
    assert_eq!(format_unix_secs(0), "1970-01-01T00:00:00Z");
}

#[test]
fn known_instant_formats_correctly() {
    assert_eq!(format_unix_secs(1_709_641_696), "2024-03-05T12:28:16Z");
}

// Bucket layout: `.conf/audit/<YYYY-MM>/<20-digit millis>-<8 hex>.json`, one
// month prefix per bucket. Hardcoded rather than imported because the layout
// itself is the contract: backup prune and the recent-list walk both read it.
const AUDIT_PREFIX: &str = ".conf/audit/";

fn now() -> std::time::Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
}

fn month_of(secs: u64) -> String {
    format_unix_secs(secs)[..7].to_string()
}

fn month_collection(month: &str) -> Collection {
    Collection::new_owned(format!(".conf/audit/{month}"))
}

fn entry(target: &str) -> AuditEntry {
    AuditEntry {
        at: format_unix_secs(now().as_secs()),
        actor: "alice".to_string(),
        action: "repo.create".to_string(),
        target: target.to_string(),
        detail: None,
    }
}

fn months_back((y, mo): (i64, u32), back: u32) -> String {
    let total = y * 12 + i64::from(mo) - 1 - i64::from(back);
    format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1)
}

#[tokio::test]
async fn record_writes_under_the_current_month_with_a_chronological_name() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let before = now();
    record(backend.as_ref(), &cipher, "alice", "repo.create", "alice/one", None).await;
    let after = now();

    let keys = backend.list_prefix(AUDIT_PREFIX).await.unwrap();
    assert_eq!(keys.len(), 1);
    let rest = keys[0].0.strip_prefix(AUDIT_PREFIX).unwrap();
    let (month, name) = rest.split_once('/').unwrap();
    assert!(
        month == month_of(before.as_secs()) || month == month_of(after.as_secs()),
        "{month}"
    );
    let (millis, suffix) = name.strip_suffix(".json").unwrap().split_once('-').unwrap();
    assert_eq!(millis.len(), 20);
    let millis: u128 = millis.parse().unwrap();
    assert!((before.as_millis()..=after.as_millis()).contains(&millis));
    assert!(suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[tokio::test]
async fn records_show_up_in_list_recent_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    // Writes two entries directly under explicit, deterministically distinct
    // millis instead of two real `record()` calls a sleep apart, since two
    // calls landing in the same wall-clock millisecond under parallel test
    // load would tie-break on the random suffix and make "newest first" flaky.
    let now = now();
    let coll = month_collection(&month_of(now.as_secs()));
    for (millis, target) in [(now.as_millis(), "alice/one"), (now.as_millis() + 1, "alice/two")] {
        coll.put(
            backend.as_ref(),
            &cipher,
            &format!("{millis:020}-00000000"),
            &entry(target),
            None,
        )
        .await
        .unwrap();
    }

    let entries = list_recent(&backend, &cipher).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].target, "alice/two");
    assert_eq!(entries[1].target, "alice/one");
}

// A full month-walk (LIST_LIMIT = 200 entries found) must stop there and not
// fall through to the whole-prefix scan, which a legacy flat key sorting
// above every month would win. Eleven months back is the walk's last step
// and crosses a January -> December year boundary except when run in
// December.
#[tokio::test]
async fn list_recent_walks_back_month_by_month_across_the_year_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let current = month_of(now().as_secs());
    let (y, mo) = current.split_once('-').unwrap();
    let old_month = months_back((y.parse().unwrap(), mo.parse().unwrap()), 11);
    let coll = month_collection(&old_month);
    for i in 0..200u32 {
        coll.put(
            backend.as_ref(),
            &cipher,
            &format!("{i:020}-00000000"),
            &entry("walked"),
            None,
        )
        .await
        .unwrap();
    }
    Collection::new_owned(".conf/audit".to_string())
        .put(
            backend.as_ref(),
            &cipher,
            "99999999999999999999-00000000",
            &entry("legacy"),
            None,
        )
        .await
        .unwrap();

    let entries = list_recent(&backend, &cipher).await.unwrap();
    assert_eq!(entries.len(), 200);
    assert!(entries.iter().all(|e| e.target == "walked"));
}

// Fewer than LIST_LIMIT entries in the walked window falls back to one
// whole-prefix LIST, which must still surface entries written before month
// buckets existed (flat `.conf/audit/<millis>-<rand>.json`).
#[tokio::test]
async fn list_recent_still_finds_legacy_flat_entries() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    Collection::new_owned(".conf/audit".to_string())
        .put(
            backend.as_ref(),
            &cipher,
            "00000000000000001000-00000000",
            &entry("legacy"),
            None,
        )
        .await
        .unwrap();
    record(backend.as_ref(), &cipher, "alice", "repo.create", "alice/one", None).await;

    let targets: Vec<_> = list_recent(&backend, &cipher)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.target)
        .collect();
    assert_eq!(targets, ["alice/one", "legacy"]);
}

#[tokio::test]
async fn list_recent_for_filters_by_target() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    record(backend.as_ref(), &cipher, "alice", "repo.create", "alice/one", None).await;
    record(backend.as_ref(), &cipher, "bob", "repo.create", "bob/two", None).await;

    let entries = list_recent_for(&backend, &cipher, "alice/one").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].actor, "alice");
}
