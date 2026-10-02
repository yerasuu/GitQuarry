//! `domain::repo::update_search_index` — builds/updates a repo's stored
//! `storage::search_index::SearchIndex` from real git history.

use gitquarry_core::domain::repo::update_search_index;
use gitquarry_core::storage::search_index::get_search_index;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::Work;

#[tokio::test]
async fn first_build_indexes_the_current_tree_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = Work::new().await;
    work.commit(&[("a.txt", "hello\n")], "add a.txt").await;
    work.commit(&[("b.txt", "world\n")], "add b.txt").await;
    let git_dir = work.git_dir();

    update_search_index(&ctx, &uid, &git_dir).await;

    let (index, _etag) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    let index = index.unwrap();
    assert!(index.last_indexed_sha.is_some());
    assert!(index.files.contains(&"a.txt".to_string()));
    assert!(index.files.contains(&"b.txt".to_string()));
    assert_eq!(index.commits.len(), 2);
    // Newest-first.
    assert_eq!(index.commits[0].subject, "add b.txt");
    assert_eq!(index.commits[1].subject, "add a.txt");
}

#[tokio::test]
async fn a_later_push_appends_only_the_new_commits_and_refreshes_the_file_list() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = Work::new().await;
    work.commit(&[("a.txt", "hello\n")], "add a.txt").await;
    let git_dir = work.git_dir();
    update_search_index(&ctx, &uid, &git_dir).await;

    work.commit(&[("b.txt", "world\n")], "add b.txt").await;
    update_search_index(&ctx, &uid, &git_dir).await;

    let (index, _etag) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    let index = index.unwrap();
    assert!(index.files.contains(&"a.txt".to_string()));
    assert!(index.files.contains(&"b.txt".to_string()));
    // Both commits present — the second call only had to walk the new one,
    // but the old one's already-stored entry is retained, not dropped.
    assert_eq!(index.commits.len(), 2);
    assert_eq!(index.commits[0].subject, "add b.txt");
    assert_eq!(index.commits[1].subject, "add a.txt");
}

#[tokio::test]
async fn re_indexing_at_the_same_head_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = Work::new().await;
    work.commit(&[("a.txt", "hello\n")], "add a.txt").await;
    let git_dir = work.git_dir();
    update_search_index(&ctx, &uid, &git_dir).await;
    let (_index, etag_after_first) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();

    // No new commits since the last build — must not touch storage again.
    update_search_index(&ctx, &uid, &git_dir).await;
    let (_index, etag_after_second) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    assert_eq!(etag_after_first, etag_after_second);
}

#[tokio::test]
async fn empty_repo_is_left_unindexed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = Work::new().await;
    let git_dir = work.git_dir();

    update_search_index(&ctx, &uid, &git_dir).await;

    let (index, _etag) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    assert!(index.is_none());
}
