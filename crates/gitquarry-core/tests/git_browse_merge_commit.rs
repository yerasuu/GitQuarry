//! `git::browse::show_commit`/`diffstat` on a **merge** commit.
//!
//! Plain `git show` prints no patch at all for a commit with more than one
//! parent — git won't guess which parent to diff against. Every merge
//! therefore rendered as an empty commit page, including the ones this app
//! creates itself every time a pull request is merged.

use gitquarry_core::git::browse::{diffstat, show_commit};

/// A repo whose `main` has a real merge commit at its tip: `main` and
/// `feature` each add their own file, then `main` merges `feature`.
/// Deliberately a *clean* merge — that is the case the combined diff
/// (`--cc`) would still render empty, and the overwhelmingly common one.
fn repo_with_a_merge() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().to_path_buf();
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(&work)
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
    std::fs::write(work.join("base.txt"), "base\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "base"]);

    run(&["switch", "--quiet", "-c", "feature"]);
    std::fs::write(work.join("from-feature.txt"), "feature side\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "feature work"]);

    run(&["switch", "--quiet", "main"]);
    std::fs::write(work.join("from-main.txt"), "main side\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "main work"]);
    run(&["merge", "--quiet", "--no-ff", "-m", "Merge pull request #1", "feature"]);

    let sha = String::from_utf8(
        std::process::Command::new("git")
            .current_dir(&work)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    (dir, sha)
}

#[tokio::test]
async fn a_merge_commit_shows_what_it_brought_onto_the_branch() {
    let (dir, sha) = repo_with_a_merge();
    let git_dir = dir.path().join(".git");

    let (info, patch) = show_commit(&git_dir, &sha).await.unwrap();

    assert_eq!(info.subject, "Merge pull request #1");
    assert!(
        patch.contains("from-feature.txt"),
        "a merge commit rendered with no patch at all: {patch:?}"
    );
    assert!(patch.contains("feature side"), "{patch}");
    // The first parent's own contribution is *not* part of what the merge
    // brought in — it was already on the branch before it.
    assert!(!patch.contains("from-main.txt"), "{patch}");
}

/// The stat above the patch has to count the same files the patch shows,
/// or a merge page claims changes it then doesn't render.
#[tokio::test]
async fn a_merge_commits_stat_matches_its_patch() {
    let (dir, sha) = repo_with_a_merge();
    let git_dir = dir.path().join(".git");

    let stat = diffstat(&git_dir, &sha).await.unwrap();

    assert!(stat.contains("from-feature.txt"), "{stat}");
    assert!(!stat.contains("from-main.txt"), "{stat}");
}

/// An ordinary single-parent commit is unaffected: `-m --first-parent`
/// changes nothing when there is only one parent to diff against.
#[tokio::test]
async fn an_ordinary_commit_still_shows_its_own_change() {
    let (dir, _) = repo_with_a_merge();
    let git_dir = dir.path().join(".git");
    let sha = String::from_utf8(
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["rev-parse", "HEAD^"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let (info, patch) = show_commit(&git_dir, &sha).await.unwrap();

    assert_eq!(info.subject, "main work");
    assert!(patch.contains("from-main.txt"), "{patch}");
}
