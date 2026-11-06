use gitquarry_core::git::*;
use gitquarry_test_support::git;
use std::path::Path;

/// `git` with `stdin` piped in, kept local because the shared `git` can't
/// feed `hash-object --stdin`/`mktree`.
fn git_with_stdin(git_dir: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// One commit whose tree is exactly `files` (name -> content) — each
/// call gives the full tree, not a diff from the parent, so callers can
/// build genuinely diverging branches (different files touched, or the
/// same file with non-overlapping edits) instead of a trivial
/// fast-forward.
async fn commit_with_files(git_dir: &Path, parent: Option<&str>, files: &[(&str, &str)], message: &str) -> String {
    let mut tree_input = String::new();
    for (name, content) in files {
        let blob = git_with_stdin(git_dir, &["hash-object", "-w", "--stdin"], content);
        tree_input.push_str(&format!("100644 blob {blob}\t{name}\n"));
    }
    let tree = git_with_stdin(git_dir, &["mktree"], &tree_input);
    let mut args = vec!["commit-tree", tree.as_str()];
    if let Some(p) = parent {
        args.push("-p");
        args.push(p);
    }
    args.push("-m");
    args.push(message);
    git(git_dir, &args).await
}

#[tokio::test]
async fn merge_tree_is_clean_when_the_branches_touch_different_files() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let base = commit_with_files(&git_dir, None, &[("f.txt", "line1\n")], "base").await;
    let ours = commit_with_files(
        &git_dir,
        Some(&base),
        &[("f.txt", "line1\n"), ("g.txt", "ours file\n")],
        "ours",
    )
    .await;
    let theirs = commit_with_files(&git_dir, Some(&base), &[("f.txt", "line1\nmore\n")], "theirs").await;

    match merge_tree(&git_dir, &ours, &theirs, None).await.unwrap() {
        MergeTreeOutcome::Clean(tree) => assert!(!tree.is_empty()),
        MergeTreeOutcome::Conflicted(paths) => panic!("expected a clean merge, got conflicts: {paths:?}"),
    }
}

#[tokio::test]
async fn merge_tree_reports_conflicted_paths() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let base = commit_with_files(&git_dir, None, &[("f.txt", "line one\n")], "base").await;
    let ours = commit_with_files(&git_dir, Some(&base), &[("f.txt", "ours version\n")], "ours").await;
    let theirs = commit_with_files(&git_dir, Some(&base), &[("f.txt", "theirs version\n")], "theirs").await;

    match merge_tree(&git_dir, &ours, &theirs, None).await.unwrap() {
        MergeTreeOutcome::Conflicted(paths) => assert_eq!(paths, vec!["f.txt".to_string()]),
        MergeTreeOutcome::Clean(tree) => panic!("expected a conflict, got a clean tree: {tree}"),
    }
}

#[tokio::test]
async fn commit_tree_writes_a_commit_with_the_given_parents_and_message() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let base = commit_with_files(&git_dir, None, &[("f.txt", "line1\n")], "base").await;
    let ours = commit_with_files(
        &git_dir,
        Some(&base),
        &[("f.txt", "line1\n"), ("g.txt", "ours file\n")],
        "ours",
    )
    .await;
    let theirs = commit_with_files(&git_dir, Some(&base), &[("f.txt", "line1\nmore\n")], "theirs").await;

    let tree = match merge_tree(&git_dir, &ours, &theirs, None).await.unwrap() {
        MergeTreeOutcome::Clean(tree) => tree,
        MergeTreeOutcome::Conflicted(paths) => panic!("unexpected conflict: {paths:?}"),
    };

    // A leading `-` in the message must reach `-m` as its value, not get
    // parsed as a new flag.
    let identity = ("tester", "tester@example.com", "1700000000 +0000");
    let merge_sha = commit_tree(
        &git_dir,
        &tree,
        &[&ours, &theirs],
        "-merge message",
        identity,
        identity,
        None,
    )
    .await
    .unwrap();
    assert_eq!(merge_sha.len(), 40);

    let parents = git(&git_dir, &["log", "-1", "--format=%P", &merge_sha]).await;
    assert!(parents.contains(&ours[..8]), "{parents}");
    assert!(parents.contains(&theirs[..8]), "{parents}");
    let subject = git(&git_dir, &["log", "-1", "--format=%s", &merge_sha]).await;
    assert_eq!(subject, "-merge message");
}

#[tokio::test]
async fn oversized_new_blobs_flags_only_the_blob_over_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let sha = commit_with_files(
        &git_dir,
        None,
        &[("small.txt", "hi\n"), ("big.bin", &"x".repeat(200))],
        "init",
    )
    .await;

    let oversized = oversized_new_blobs(&git_dir, &sha, &[], 100).await.unwrap();
    assert_eq!(oversized.len(), 1);
    assert_eq!(oversized[0].0, "big.bin");
    assert_eq!(oversized[0].1, 200);
}

#[tokio::test]
async fn oversized_new_blobs_is_empty_under_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let sha = commit_with_files(&git_dir, None, &[("f.txt", "hello\n")], "init").await;

    let oversized = oversized_new_blobs(&git_dir, &sha, &[], 100).await.unwrap();
    assert!(oversized.is_empty());
}

#[tokio::test]
async fn oversized_new_blobs_ignores_a_blob_already_covered_by_have_tips() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let base = commit_with_files(&git_dir, None, &[("big.bin", &"x".repeat(200))], "base").await;
    let next = commit_with_files(&git_dir, Some(&base), &[("big.bin", &"x".repeat(200))], "next").await;

    // `base` already has the oversized blob; a push moving the tip from
    // `base` to `next` introduces no new blob at all.
    let oversized = oversized_new_blobs(&git_dir, &next, &[base], 100).await.unwrap();
    assert!(oversized.is_empty());
}
