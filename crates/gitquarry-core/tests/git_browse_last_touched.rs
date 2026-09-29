use gitquarry_core::git::browse::*;
use gitquarry_test_support::Work;

#[tokio::test]
async fn resolves_each_root_entry_to_the_commit_that_last_touched_it() {
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n"), ("sub/b.txt", "one\n")], "add a.txt and sub/b.txt")
        .await;
    work.commit(&[("a.txt", "two\n")], "update a.txt").await;

    let git_dir = work.git_dir();
    let entries = vec!["a.txt".to_string(), "sub".to_string()];
    let result = last_touched(&git_dir, "main", "", &entries).await.unwrap();

    assert_eq!(result["a.txt"].subject, "update a.txt");
    assert_eq!(result["sub"].subject, "add a.txt and sub/b.txt");
}

#[tokio::test]
async fn resolves_entries_scoped_to_a_subdirectory() {
    let work = Work::new().await;
    work.commit(
        &[("sub/b.txt", "one\n"), ("sub/c.txt", "one\n")],
        "add sub/b.txt and sub/c.txt",
    )
    .await;
    work.commit(&[("sub/c.txt", "two\n")], "update sub/c.txt").await;

    let git_dir = work.git_dir();
    let entries = vec!["b.txt".to_string(), "c.txt".to_string()];
    let result = last_touched(&git_dir, "main", "sub", &entries).await.unwrap();

    assert_eq!(result["b.txt"].subject, "add sub/b.txt and sub/c.txt");
    assert_eq!(result["c.txt"].subject, "update sub/c.txt");
}

#[tokio::test]
async fn leaves_untouched_entries_out_of_the_result() {
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n")], "add a.txt").await;

    let git_dir = work.git_dir();
    let entries = vec!["a.txt".to_string(), "does-not-exist.txt".to_string()];
    let result = last_touched(&git_dir, "main", "", &entries).await.unwrap();

    assert!(result.contains_key("a.txt"));
    assert!(!result.contains_key("does-not-exist.txt"));
}

#[tokio::test]
async fn empty_entries_returns_empty_map() {
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n")], "add a.txt").await;

    let git_dir = work.git_dir();
    let result = last_touched(&git_dir, "main", "", &[]).await.unwrap();
    assert!(result.is_empty());
}
