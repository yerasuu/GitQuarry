//! `git::browse::show_commit`/`diffstat` on a **merge** commit.
//!
//! Plain `git show` prints no patch at all for a commit with more than one
//! parent — git won't guess which parent to diff against. Every merge
//! therefore rendered as an empty commit page, including the ones this app
//! creates itself every time a merge request is merged.

use gitquarry_core::git::browse::{diffstat, show_commit};
use gitquarry_test_support::{Work, git};

/// A repo whose `main` has a real merge commit at its tip: `main` and
/// `feature` each add their own file, then `main` merges `feature`.
/// Deliberately a *clean* merge — that is the case the combined diff
/// (`--cc`) would still render empty, and the overwhelmingly common one.
async fn repo_with_a_merge() -> (Work, String) {
    let work = Work::new().await;
    work.commit(&[("base.txt", "base\n")], "base").await;

    git(work.path(), &["switch", "--quiet", "-c", "feature"]).await;
    work.commit(&[("from-feature.txt", "feature side\n")], "feature work")
        .await;

    git(work.path(), &["switch", "--quiet", "main"]).await;
    work.commit(&[("from-main.txt", "main side\n")], "main work").await;
    git(
        work.path(),
        &["merge", "--quiet", "--no-ff", "-m", "Merge merge request #1", "feature"],
    )
    .await;

    let sha = git(work.path(), &["rev-parse", "HEAD"]).await;
    (work, sha)
}

#[tokio::test]
async fn a_merge_commit_shows_what_it_brought_onto_the_branch() {
    let (work, sha) = repo_with_a_merge().await;
    let git_dir = work.git_dir();

    let (info, patch) = show_commit(&git_dir, &sha).await.unwrap();

    assert_eq!(info.subject, "Merge merge request #1");
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
    let (work, sha) = repo_with_a_merge().await;
    let git_dir = work.git_dir();

    let stat = diffstat(&git_dir, &sha).await.unwrap();

    assert!(stat.contains("from-feature.txt"), "{stat}");
    assert!(!stat.contains("from-main.txt"), "{stat}");
}

/// An ordinary single-parent commit is unaffected: `-m --first-parent`
/// changes nothing when there is only one parent to diff against.
#[tokio::test]
async fn an_ordinary_commit_still_shows_its_own_change() {
    let (work, _) = repo_with_a_merge().await;
    let git_dir = work.git_dir();
    let sha = git(work.path(), &["rev-parse", "HEAD^"]).await;

    let (info, patch) = show_commit(&git_dir, &sha).await.unwrap();

    assert_eq!(info.subject, "main work");
    assert!(patch.contains("from-main.txt"), "{patch}");
}
