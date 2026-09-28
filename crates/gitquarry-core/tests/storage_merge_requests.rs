use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::merge_requests::*;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{Backend, Cas};
use std::sync::Arc;

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    });
    let cipher = Cipher::derive("test passphrase", &[3u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

fn sample(number: u64) -> MergeRequestMeta {
    MergeRequestMeta {
        number,
        title: "Add widget".to_string(),
        description: "does a thing".to_string(),
        source_repo: Uid::from("team/proj".to_string()),
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        status: MrStatus::Open,
        author: "alice".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        merged_at: None,
        merged_by: None,
        merge_commit_sha: None,
        is_draft: false,
        labels: Vec::new(),
        assignees: Vec::new(),
        requested_reviewers: Vec::new(),
    }
}

#[tokio::test]
async fn next_mr_number_starts_at_one_and_increments() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    assert_eq!(next_mr_number(&backend, &cipher, &uid).await.unwrap(), 1);
    assert_eq!(next_mr_number(&backend, &cipher, &uid).await.unwrap(), 2);
    assert_eq!(next_mr_number(&backend, &cipher, &uid).await.unwrap(), 3);
}

#[tokio::test]
async fn next_mr_number_is_scoped_per_repo_uid() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let a = Uid::from("team/a".to_string());
    let b = Uid::from("team/b".to_string());

    assert_eq!(next_mr_number(&backend, &cipher, &a).await.unwrap(), 1);
    assert_eq!(next_mr_number(&backend, &cipher, &a).await.unwrap(), 2);
    // A different repo's counter starts fresh, unaffected by `a`'s.
    assert_eq!(next_mr_number(&backend, &cipher, &b).await.unwrap(), 1);
}

#[tokio::test]
async fn put_then_get_roundtrips_and_conflicts_on_a_stale_etag() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    let mr = sample(1);

    assert_eq!(
        put_merge_request(&backend, &cipher, &uid, &mr, None).await.unwrap(),
        Cas::Ok
    );
    let (fetched, etag) = get_merge_request(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(fetched.unwrap().title, "Add widget");
    assert!(etag.is_some());

    // Re-creating without the fresh etag must not silently overwrite.
    assert_eq!(
        put_merge_request(&backend, &cipher, &uid, &mr, None).await.unwrap(),
        Cas::Conflict
    );
}

#[tokio::test]
async fn get_is_none_for_a_never_created_number() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    assert!(
        get_merge_request(&backend, &cipher, &uid, 99)
            .await
            .unwrap()
            .0
            .is_none()
    );
}

#[tokio::test]
async fn list_merge_requests_excludes_the_counter_and_sorts_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    for number in [1, 2, 3] {
        let n = next_mr_number(backend.as_ref(), &cipher, &uid).await.unwrap();
        assert_eq!(n, number);
        put_merge_request(backend.as_ref(), &cipher, &uid, &sample(number), None)
            .await
            .unwrap();
    }

    let mrs = list_merge_requests(&backend, &cipher, &uid).await.unwrap();
    assert_eq!(mrs.iter().map(|mr| mr.number).collect::<Vec<_>>(), vec![3, 2, 1]);
}

#[tokio::test]
async fn comments_list_empty_before_any_are_added() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());
    assert_eq!(list_mr_comments(&backend, &cipher, &uid, 1).await.unwrap(), Vec::new());
}

#[tokio::test]
async fn comments_roundtrip_in_chronological_order() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    for (author, body) in [("alice", "first"), ("bob", "second"), ("alice", "third")] {
        let comment = MrComment {
            author: author.to_string(),
            body: body.to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            ..Default::default()
        };
        add_mr_comment(backend.as_ref(), &cipher, &uid, 1, &comment)
            .await
            .unwrap();
    }

    let comments = list_mr_comments(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(
        comments.iter().map(|c| c.body.as_str()).collect::<Vec<_>>(),
        vec!["first", "second", "third"]
    );
}

#[tokio::test]
async fn comments_are_scoped_per_mr_number() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    let comment = MrComment {
        author: "alice".to_string(),
        body: "on mr 1".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    add_mr_comment(backend.as_ref(), &cipher, &uid, 1, &comment)
        .await
        .unwrap();

    assert_eq!(list_mr_comments(&backend, &cipher, &uid, 1).await.unwrap().len(), 1);
    assert_eq!(list_mr_comments(&backend, &cipher, &uid, 2).await.unwrap().len(), 0);
}

#[tokio::test]
async fn list_with_keys_gives_every_comment_a_distinct_stable_key() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    for body in ["first", "second"] {
        let comment = MrComment {
            author: "alice".to_string(),
            body: body.to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            ..Default::default()
        };
        add_mr_comment(backend.as_ref(), &cipher, &uid, 1, &comment)
            .await
            .unwrap();
    }

    let with_keys = list_mr_comments_with_keys(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(with_keys.len(), 2);
    assert_ne!(with_keys[0].0, with_keys[1].0);
    assert_eq!(with_keys[0].1.body, "first");
    assert_eq!(with_keys[1].1.body, "second");
}

#[tokio::test]
async fn reviews_roundtrip_in_chronological_order_scoped_per_mr() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    for (author, verdict) in [
        ("bob", ReviewVerdict::ChangesRequested),
        ("carol", ReviewVerdict::Approved),
    ] {
        let review = MrReview {
            author: author.to_string(),
            verdict,
            body: String::new(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        add_mr_review(backend.as_ref(), &cipher, &uid, 1, &review)
            .await
            .unwrap();
    }

    let reviews = list_mr_reviews(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(
        reviews.iter().map(|r| r.author.as_str()).collect::<Vec<_>>(),
        vec!["bob", "carol"]
    );
    assert_eq!(reviews[0].verdict, ReviewVerdict::ChangesRequested);
    assert!(list_mr_reviews(&backend, &cipher, &uid, 2).await.unwrap().is_empty());
}

#[tokio::test]
async fn reviews_and_comments_are_separate_threads() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    add_mr_comment(
        backend.as_ref(),
        &cipher,
        &uid,
        1,
        &MrComment {
            author: "alice".to_string(),
            body: "chatter".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    add_mr_review(
        backend.as_ref(),
        &cipher,
        &uid,
        1,
        &MrReview {
            author: "bob".to_string(),
            verdict: ReviewVerdict::Approved,
            body: "lgtm".to_string(),
            created_at: "2024-01-01T00:00:01Z".to_string(),
        },
    )
    .await
    .unwrap();

    assert_eq!(list_mr_comments(&backend, &cipher, &uid, 1).await.unwrap().len(), 1);
    assert_eq!(list_mr_reviews(&backend, &cipher, &uid, 1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn origin_field_roundtrips_and_defaults_to_none() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    let local = MrComment {
        author: "alice".to_string(),
        body: "local".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        origin: None,
        ..Default::default()
    };
    let imported = MrComment {
        author: "bob".to_string(),
        body: "from github".to_string(),
        created_at: "2024-01-01T00:00:01Z".to_string(),
        origin: Some("m1".to_string()),
        ..Default::default()
    };
    add_mr_comment(backend.as_ref(), &cipher, &uid, 1, &local)
        .await
        .unwrap();
    add_mr_comment(backend.as_ref(), &cipher, &uid, 1, &imported)
        .await
        .unwrap();

    let comments = list_mr_comments(&backend, &cipher, &uid, 1).await.unwrap();
    assert_eq!(comments.iter().filter(|c| c.origin.is_none()).count(), 1);
    assert_eq!(comments.iter().filter(|c| c.origin.as_deref() == Some("m1")).count(), 1);
}

#[tokio::test]
async fn an_anchor_roundtrips_alongside_plain_thread_comments() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let uid = Uid::from("team/proj".to_string());

    let plain = MrComment {
        author: "alice".to_string(),
        body: "thread".to_string(),
        ..Default::default()
    };
    let anchored = MrComment {
        author: "bob".to_string(),
        body: "on this line".to_string(),
        anchor: Some(CommentAnchor {
            path: "src/main.rs".to_string(),
            side: DiffSide::Old,
            line: 42,
        }),
        ..Default::default()
    };
    add_mr_comment(backend.as_ref(), &cipher, &uid, 1, &plain)
        .await
        .unwrap();
    add_mr_comment(backend.as_ref(), &cipher, &uid, 1, &anchored)
        .await
        .unwrap();

    let comments = list_mr_comments(&backend, &cipher, &uid, 1).await.unwrap();
    assert!(comments[0].anchor.is_none());
    let anchor = comments[1].anchor.as_ref().unwrap();
    assert_eq!(anchor.path, "src/main.rs");
    assert_eq!(anchor.side, DiffSide::Old);
    assert_eq!(anchor.line, 42);
}
