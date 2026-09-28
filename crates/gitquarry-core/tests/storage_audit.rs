use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::audit::*;
use s3db::LocalBackend;
use s3db_crypto::SALT_LEN;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn epoch_formats_as_1970() {
    assert_eq!(format_unix_secs(0), "1970-01-01T00:00:00Z");
}

#[test]
fn known_instant_formats_correctly() {
    assert_eq!(format_unix_secs(1_709_641_696), "2024-03-05T12:28:16Z");
}

#[test]
fn entry_names_sort_chronologically() {
    let a = entry_name(1_000);
    let b = entry_name(2_000);
    assert!(a < b);
}

#[test]
fn month_bucket_formats_year_dash_month() {
    assert_eq!(month_bucket(1_709_641_696), "2024-03");
}

#[test]
fn prev_month_wraps_year_boundary() {
    assert_eq!(prev_month((2024, 1, 15)), (2023, 12, 15));
    assert_eq!(prev_month((2024, 3, 15)), (2024, 2, 15));
}

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    });
    let cipher = Cipher::derive("test passphrase", &[7u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

#[tokio::test]
async fn records_show_up_in_list_recent_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher): (_, Cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    // Writes two entries directly under explicit, deterministically
    // distinct millis instead of two real `record()` calls a sleep
    // apart -- `entry_key`'s ordering is millis-then-random-suffix (see
    // its own doc comment), so two calls racing to land in the very
    // same wall-clock millisecond under heavy parallel test load could
    // tie on the random suffix instead, making the "newest first"
    // assertion below flaky regardless of how generous the sleep is.
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let older = AuditEntry {
        at: format_unix_secs(now.as_secs()),
        actor: "alice".to_string(),
        action: "repo.create".to_string(),
        target: "alice/one".to_string(),
        detail: None,
    };
    let newer = AuditEntry {
        target: "alice/two".to_string(),
        ..older.clone()
    };
    let coll = audit_collection(&month_bucket(now.as_secs()));
    coll.put(backend.as_ref(), &cipher, &entry_name(now.as_millis()), &older, None)
        .await
        .unwrap();
    coll.put(
        backend.as_ref(),
        &cipher,
        &entry_name(now.as_millis() + 1),
        &newer,
        None,
    )
    .await
    .unwrap();

    let entries = list_recent(&backend, &cipher).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].target, "alice/two"); // newest first
    assert_eq!(entries[1].target, "alice/one");
}

#[tokio::test]
async fn list_recent_for_filters_by_target() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher): (_, Cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    record(backend.as_ref(), &cipher, "alice", "repo.create", "alice/one", None).await;
    record(backend.as_ref(), &cipher, "bob", "repo.create", "bob/two", None).await;

    let entries = list_recent_for(&backend, &cipher, "alice/one").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].actor, "alice");
}
