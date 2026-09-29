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
    gitquarry_test_support::git(dir.path(), &["init", "--quiet", "--bare"]).await;

    assert_eq!(resolve_head(dir.path()).await.unwrap(), None);
}

#[tokio::test]
async fn resolve_commit_dereferences_an_annotated_tag_to_its_commit_sha() {
    let work = gitquarry_test_support::Work::new().await;
    let commit_sha = work.commit(&[("f", "x")], "c").await;
    gitquarry_test_support::git(work.path(), &["tag", "-a", "v1.0", "-m", "annotated"]).await;
    let git_dir = work.git_dir();

    assert_eq!(
        resolve_commit(&git_dir, "v1.0").await.unwrap(),
        Some(commit_sha.clone())
    );
    assert_eq!(resolve_commit(&git_dir, "main").await.unwrap(), Some(commit_sha));
    assert_eq!(resolve_commit(&git_dir, "does-not-exist").await.unwrap(), None);
}
