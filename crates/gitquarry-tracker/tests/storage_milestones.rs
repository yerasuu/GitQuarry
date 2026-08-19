use gitquarry_tracker::storage::milestones::*;
use gitquarry_wire::Uid;
use s3db::LocalBackend;
use s3db::{Backend, Cas};
use s3db::{LocalEngine, LocalParams};
use s3db_crypto::{Cipher, SALT_LEN};
use std::sync::Arc;

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = Cipher::derive("test passphrase", &[19u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

fn sample(number: u64) -> MilestoneMeta {
    MilestoneMeta {
        number,
        title: "v1.0".to_string(),
        description: "first stable release".to_string(),
        due_date: "2026-12-01".to_string(),
        status: MilestoneStatus::Open,
        open_issues: 0,
        closed_issues: 0,
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
    }
}

#[tokio::test]
async fn next_milestone_number_starts_at_one_and_increments() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    assert_eq!(next_milestone_number(&backend, &cipher, &uid).await.unwrap(), 1);
    assert_eq!(next_milestone_number(&backend, &cipher, &uid).await.unwrap(), 2);
    assert_eq!(next_milestone_number(&backend, &cipher, &uid).await.unwrap(), 3);
}

#[tokio::test]
async fn next_milestone_number_is_scoped_per_repo_uid() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let a = Uid::from("team/a".to_string());
    let b = Uid::from("team/b".to_string());

    assert_eq!(next_milestone_number(&backend, &cipher, &a).await.unwrap(), 1);
    assert_eq!(next_milestone_number(&backend, &cipher, &a).await.unwrap(), 2);
    assert_eq!(next_milestone_number(&backend, &cipher, &b).await.unwrap(), 1);
}

#[tokio::test]
async fn put_then_get_roundtrips_and_conflicts_on_a_stale_etag() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    let milestone = sample(1);

    assert_eq!(
        put_milestone(&backend, &cipher, &uid, &milestone, None).await.unwrap(),
        Cas::Ok
    );
    let (fetched, etag) = get_milestone(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(fetched.unwrap().title, "v1.0");
    assert!(etag.is_some());

    assert_eq!(
        put_milestone(&backend, &cipher, &uid, &milestone, None).await.unwrap(),
        Cas::Conflict
    );
}

#[tokio::test]
async fn get_is_none_for_a_never_created_number() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    assert!(get_milestone(&backend, &cipher, &uid, 99).await.unwrap().0.is_none());
}

#[tokio::test]
async fn list_milestones_excludes_the_counter_and_sorts_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    for number in [1, 2, 3] {
        let n = next_milestone_number(backend.as_ref(), &cipher, &uid).await.unwrap();
        assert_eq!(n, number);
        put_milestone(backend.as_ref(), &cipher, &uid, &sample(number), None)
            .await
            .unwrap();
    }

    let milestones = list_milestones(&backend, &cipher, &uid).await.unwrap();
    assert_eq!(milestones.iter().map(|m| m.number).collect::<Vec<_>>(), vec![3, 2, 1]);
}
