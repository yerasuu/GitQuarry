use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::uid::Uid;
use gitquarry_mirror_external::access::*;

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = Cipher::derive("test passphrase", &[7u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

fn sample_mirror(id: &str) -> ExternalMirror {
    ExternalMirror {
        id: id.to_string(),
        remote_url: "https://github.com/acme/demo.git".to_string(),
        token: "ghp_secret".to_string(),
        provider: ExternalMirrorProvider::GitHub,
        direction: SyncDirection::Bidirectional,
        enabled: true,
        label: "backup".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn set_get_list_and_delete_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let mirror = sample_mirror("m1");
    set_external_mirror(&backend, &cipher, &uid, &mirror, None)
        .await
        .unwrap();

    let (fetched, etag) = get_external_mirror(&backend, &cipher, &uid, "m1").await.unwrap();
    assert_eq!(fetched.unwrap().remote_url, mirror.remote_url);
    assert!(etag.is_some());

    let all = list_external_mirrors(&backend, &cipher, &uid).await.unwrap();
    assert_eq!(all.len(), 1);

    delete_external_mirror(&backend, &cipher, &uid, "m1").await;
    let (gone, _) = get_external_mirror(&backend, &cipher, &uid, "m1").await.unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn set_external_mirror_rejects_an_invalid_url() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let mut mirror = sample_mirror("m1");
    mirror.remote_url = "not-a-url".to_string();
    assert!(
        set_external_mirror(&backend, &cipher, &uid, &mirror, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn update_last_synced_records_sha_and_marks_in_sync() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    let mirror = sample_mirror("m1");
    set_external_mirror(&backend, &cipher, &uid, &mirror, None)
        .await
        .unwrap();

    update_last_synced(&backend, &cipher, &uid, "m1", "refs/heads/main", "abc123")
        .await
        .unwrap();

    let (fetched, _) = get_external_mirror(&backend, &cipher, &uid, "m1").await.unwrap();
    let fetched = fetched.unwrap();
    assert_eq!(fetched.last_synced.get("refs/heads/main"), Some(&"abc123".to_string()));
    assert_eq!(fetched.ref_status.get("refs/heads/main"), Some(&RefSyncStatus::InSync));
}

#[tokio::test]
async fn set_ref_status_leaves_last_synced_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    let mirror = sample_mirror("m1");
    set_external_mirror(&backend, &cipher, &uid, &mirror, None)
        .await
        .unwrap();
    update_last_synced(&backend, &cipher, &uid, "m1", "refs/heads/main", "abc123")
        .await
        .unwrap();

    set_ref_status(
        &backend,
        &cipher,
        &uid,
        "m1",
        "refs/heads/main",
        RefSyncStatus::Diverged,
    )
    .await
    .unwrap();

    let (fetched, _) = get_external_mirror(&backend, &cipher, &uid, "m1").await.unwrap();
    let fetched = fetched.unwrap();
    assert_eq!(fetched.last_synced.get("refs/heads/main"), Some(&"abc123".to_string()));
    assert_eq!(
        fetched.ref_status.get("refs/heads/main"),
        Some(&RefSyncStatus::Diverged)
    );
}

#[tokio::test]
async fn pr_link_round_trips_and_lists_scoped_to_its_own_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let link = ExternalPrLink {
        local_number: 1,
        external_number: 42,
        remote_updated_cursor: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    put_pr_link(&backend, &cipher, &uid, "m1", &link, None).await.unwrap();

    let (fetched, etag) = get_pr_link(&backend, &cipher, &uid, "m1", 1).await.unwrap();
    assert_eq!(fetched.unwrap().external_number, 42);
    assert!(etag.is_some());

    // A different mirror on the same repo has its own, independent set
    // of links, even for the same local PR number.
    let other_mirror_links = list_pr_links(&backend, &cipher, &uid, "m2").await.unwrap();
    assert!(other_mirror_links.is_empty());
    let this_mirror_links = list_pr_links(&backend, &cipher, &uid, "m1").await.unwrap();
    assert_eq!(this_mirror_links.len(), 1);
}

#[tokio::test]
async fn a_whole_mirror_failure_is_recorded_and_cleared_on_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    set_external_mirror(&backend, &cipher, &uid, &sample_mirror("m1"), None)
        .await
        .unwrap();
    assert!(
        get_external_mirror(&backend, &cipher, &uid, "m1")
            .await
            .unwrap()
            .0
            .unwrap()
            .last_error
            .is_none()
    );

    set_mirror_error(
        &backend,
        &cipher,
        &uid,
        "m1",
        Some("couldn't reach the remote".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(
        get_external_mirror(&backend, &cipher, &uid, "m1")
            .await
            .unwrap()
            .0
            .unwrap()
            .last_error
            .as_deref(),
        Some("couldn't reach the remote")
    );

    // A later successful sync clears it, so a recovered mirror stops
    // showing a stale failure.
    set_mirror_error(&backend, &cipher, &uid, "m1", None).await.unwrap();
    assert!(
        get_external_mirror(&backend, &cipher, &uid, "m1")
            .await
            .unwrap()
            .0
            .unwrap()
            .last_error
            .is_none()
    );
}

#[tokio::test]
async fn a_recorded_failure_never_carries_an_embedded_token() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    set_external_mirror(&backend, &cipher, &uid, &sample_mirror("m1"), None)
        .await
        .unwrap();

    set_mirror_error(
        &backend,
        &cipher,
        &uid,
        "m1",
        Some("git push https://x-access-token:ghp_secret@github.com/acme/demo.git failed".to_string()),
    )
    .await
    .unwrap();

    let error = get_external_mirror(&backend, &cipher, &uid, "m1")
        .await
        .unwrap()
        .0
        .unwrap()
        .last_error
        .unwrap();
    assert!(!error.contains("ghp_secret"), "{error}");
    assert!(error.contains("https://github.com/acme/demo.git"), "{error}");
}

#[test]
fn a_far_side_error_is_capped_before_it_becomes_bucket_state() {
    // A forge's error *page* comes back verbatim in the adapters' error
    // text; stored uncapped it would sit in the bucket and be re-rendered
    // into the settings tab on every load.
    let huge = "x".repeat(10_000);
    let capped = cap_error(&huge);
    assert!(capped.len() < 600, "not capped: {} bytes", capped.len());
    assert!(capped.ends_with("… (truncated)"));
    // Short messages, which is what most of them are, pass through whole.
    assert_eq!(cap_error("fatal: repository not found"), "fatal: repository not found");
}

#[test]
fn capping_never_splits_a_character() {
    // The cut point has to be a char boundary — `&str[..n]` panics
    // otherwise, and this text comes from the far side.
    let multibyte = "é".repeat(5_000);
    let capped = cap_error(&multibyte);
    assert!(capped.ends_with("… (truncated)"));
}
