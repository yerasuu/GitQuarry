use gitquarry_tracker::storage::issues::*;
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
    let cipher = Cipher::derive("test passphrase", &[7u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

fn sample(number: u64) -> IssueMeta {
    IssueMeta {
        number,
        title: "Something is broken".to_string(),
        body: "steps to reproduce".to_string(),
        status: IssueStatus::Open,
        author: "alice".to_string(),
        assignees: Vec::new(),
        labels: Vec::new(),
        milestone: None,
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
    }
}

#[tokio::test]
async fn next_issue_number_starts_at_one_and_increments() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    assert_eq!(next_issue_number(&backend, &cipher, &uid).await.unwrap(), 1);
    assert_eq!(next_issue_number(&backend, &cipher, &uid).await.unwrap(), 2);
    assert_eq!(next_issue_number(&backend, &cipher, &uid).await.unwrap(), 3);
}

#[tokio::test]
async fn next_issue_number_is_scoped_per_repo_uid() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let a = Uid::from("team/a".to_string());
    let b = Uid::from("team/b".to_string());

    assert_eq!(next_issue_number(&backend, &cipher, &a).await.unwrap(), 1);
    assert_eq!(next_issue_number(&backend, &cipher, &a).await.unwrap(), 2);
    assert_eq!(next_issue_number(&backend, &cipher, &b).await.unwrap(), 1);
}

#[tokio::test]
async fn put_then_get_roundtrips_and_conflicts_on_a_stale_etag() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    let issue = sample(1);

    assert_eq!(put_issue(&backend, &cipher, &uid, &issue, None).await.unwrap(), Cas::Ok);
    let (fetched, etag) = get_issue(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(fetched.unwrap().title, "Something is broken");
    assert!(etag.is_some());

    assert_eq!(
        put_issue(&backend, &cipher, &uid, &issue, None).await.unwrap(),
        Cas::Conflict
    );
}

#[tokio::test]
async fn get_is_none_for_a_never_created_number() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    assert!(get_issue(&backend, &cipher, &uid, 99).await.unwrap().0.is_none());
}

#[tokio::test]
async fn list_issues_excludes_the_counter_and_sorts_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    for number in [1, 2, 3] {
        let n = next_issue_number(backend.as_ref(), &cipher, &uid).await.unwrap();
        assert_eq!(n, number);
        put_issue(backend.as_ref(), &cipher, &uid, &sample(number), None)
            .await
            .unwrap();
    }

    let issues = list_issues(&backend, &cipher, &uid).await.unwrap();
    assert_eq!(
        issues.iter().map(|issue| issue.number).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

#[tokio::test]
async fn comments_list_empty_before_any_are_added() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());
    assert_eq!(
        list_issue_comments(&backend, &cipher, &uid, 1).await.unwrap(),
        Vec::new()
    );
}

#[tokio::test]
async fn comments_roundtrip_in_chronological_order() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    for (author, body) in [("alice", "first"), ("bob", "second"), ("alice", "third")] {
        let comment = IssueComment {
            author: author.to_string(),
            body: body.to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        add_issue_comment(backend.as_ref(), &cipher, &uid, 1, &comment)
            .await
            .unwrap();
    }

    let comments = list_issue_comments(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(
        comments.iter().map(|(_id, c)| c.body.as_str()).collect::<Vec<_>>(),
        vec!["first", "second", "third"]
    );
}

#[tokio::test]
async fn comments_are_scoped_per_issue_number() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    let comment = IssueComment {
        author: "alice".to_string(),
        body: "on issue 1".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
    };
    add_issue_comment(backend.as_ref(), &cipher, &uid, 1, &comment)
        .await
        .unwrap();

    assert_eq!(list_issue_comments(&backend, &cipher, &uid, 1).await.unwrap().len(), 1);
    assert_eq!(list_issue_comments(&backend, &cipher, &uid, 2).await.unwrap().len(), 0);
}
