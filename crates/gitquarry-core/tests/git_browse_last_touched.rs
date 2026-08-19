use gitquarry_core::git::browse::*;

fn git_run(work: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(work)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[tokio::test]
async fn resolves_each_root_entry_to_the_commit_that_last_touched_it() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git_run(work, &["init", "--quiet", "--initial-branch=main"]);

    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    std::fs::create_dir(work.join("sub")).unwrap();
    std::fs::write(work.join("sub/b.txt"), "one\n").unwrap();
    git_run(work, &["add", "."]);
    git_run(work, &["commit", "--quiet", "-m", "add a.txt and sub/b.txt"]);

    std::fs::write(work.join("a.txt"), "two\n").unwrap();
    git_run(work, &["add", "."]);
    git_run(work, &["commit", "--quiet", "-m", "update a.txt"]);

    let git_dir = work.join(".git");
    let entries = vec!["a.txt".to_string(), "sub".to_string()];
    let result = last_touched(&git_dir, "main", "", &entries).await.unwrap();

    assert_eq!(result["a.txt"].subject, "update a.txt");
    assert_eq!(result["sub"].subject, "add a.txt and sub/b.txt");
}

#[tokio::test]
async fn resolves_entries_scoped_to_a_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git_run(work, &["init", "--quiet", "--initial-branch=main"]);

    std::fs::create_dir(work.join("sub")).unwrap();
    std::fs::write(work.join("sub/b.txt"), "one\n").unwrap();
    std::fs::write(work.join("sub/c.txt"), "one\n").unwrap();
    git_run(work, &["add", "."]);
    git_run(work, &["commit", "--quiet", "-m", "add sub/b.txt and sub/c.txt"]);

    std::fs::write(work.join("sub/c.txt"), "two\n").unwrap();
    git_run(work, &["add", "."]);
    git_run(work, &["commit", "--quiet", "-m", "update sub/c.txt"]);

    let git_dir = work.join(".git");
    let entries = vec!["b.txt".to_string(), "c.txt".to_string()];
    let result = last_touched(&git_dir, "main", "sub", &entries).await.unwrap();

    assert_eq!(result["b.txt"].subject, "add sub/b.txt and sub/c.txt");
    assert_eq!(result["c.txt"].subject, "update sub/c.txt");
}

#[tokio::test]
async fn leaves_untouched_entries_out_of_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git_run(work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    git_run(work, &["add", "."]);
    git_run(work, &["commit", "--quiet", "-m", "add a.txt"]);

    let git_dir = work.join(".git");
    let entries = vec!["a.txt".to_string(), "does-not-exist.txt".to_string()];
    let result = last_touched(&git_dir, "main", "", &entries).await.unwrap();

    assert!(result.contains_key("a.txt"));
    assert!(!result.contains_key("does-not-exist.txt"));
}

#[tokio::test]
async fn empty_entries_returns_empty_map() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git_run(work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    git_run(work, &["add", "."]);
    git_run(work, &["commit", "--quiet", "-m", "add a.txt"]);

    let git_dir = work.join(".git");
    let result = last_touched(&git_dir, "main", "", &[]).await.unwrap();
    assert!(result.is_empty());
}
