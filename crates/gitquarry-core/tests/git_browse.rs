use gitquarry_core::git::browse::*;

/// Regression for the `/_new` bug: a fresh `git init --bare` repo has
/// `HEAD` pointing at `refs/heads/master` (or whatever `init.defaultBranch`
/// is) via a plain symbolic ref, same as any repo — the only difference
/// is that ref is unborn (no commit yet). `symbolic-ref -q HEAD` alone
/// can't tell the two apart; `resolve_head` used to hand that unborn ref
/// straight to `handler_repo` as if it named a real commit, which then
/// failed loudly the first time anything (`ls-tree`, `log`, ...) tried
/// to actually resolve it.
#[tokio::test]
async fn resolve_head_is_none_for_a_freshly_initialized_empty_repo() {
    let dir = tempfile::tempdir().unwrap();
    let status = tokio::process::Command::new("git")
        .args(["init", "--quiet", "--bare"])
        .arg(dir.path())
        .status()
        .await
        .unwrap();
    assert!(status.success());

    assert_eq!(resolve_head(dir.path()).await.unwrap(), None);
}

#[tokio::test]
async fn resolve_commit_dereferences_an_annotated_tag_to_its_commit_sha() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    let git_dir = work.join(".git");
    let run = |args: &[&str]| {
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
    };
    run(&["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("f"), "x").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "c"]);
    run(&["tag", "-a", "v1.0", "-m", "annotated"]);

    let commits = log(&git_dir, "main", 1, None).await.unwrap();
    let commit_sha = commits[0].sha.clone();

    assert_eq!(
        resolve_commit(&git_dir, "v1.0").await.unwrap(),
        Some(commit_sha.clone())
    );
    assert_eq!(resolve_commit(&git_dir, "main").await.unwrap(), Some(commit_sha));
    assert_eq!(resolve_commit(&git_dir, "does-not-exist").await.unwrap(), None);
}
