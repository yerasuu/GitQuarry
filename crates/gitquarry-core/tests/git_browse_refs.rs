// `refs` is a private submodule of `git::browse`, re-exported via `pub use
// refs::*;` there, so the browsing module itself is the visible path.
use gitquarry_core::git::browse::*;
use gitquarry_test_support::{Work, git};

#[tokio::test]
async fn list_refs_detailed_credits_the_tagger_not_the_committer_for_an_annotated_tag() {
    let work = Work::new().await;
    std::fs::write(work.path().join("a.txt"), "one\n").unwrap();
    git(work.path(), &["add", "."]).await;
    git(
        work.path(),
        &[
            "commit",
            "--quiet",
            "--author=Alice <author@example.com>",
            "-m",
            "initial commit",
        ],
    )
    .await;
    // Kept off the shared `git` because its fixed `GIT_COMMITTER_NAME` is
    // what sets the tagger identity (env always wins over `-c`), and this
    // test needs the commit author ("Alice") and the tagger ("Bob") to be
    // genuinely different identities.
    let status = std::process::Command::new("git")
        .current_dir(work.path())
        .env("GIT_COMMITTER_NAME", "Bob")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .args(["-c", "tag.gpgsign=false", "tag", "-a", "v1.0", "-m", "release"])
        .status()
        .unwrap();
    assert!(status.success(), "git tag failed");

    let refs = list_refs_detailed(&work.git_dir(), "refs/").await.unwrap();
    let branch = refs.iter().find(|r| r.name == "refs/heads/main").unwrap();
    assert_eq!(branch.author_name, "Alice");
    let tag = refs.iter().find(|r| r.name == "refs/tags/v1.0").unwrap();
    assert_eq!(tag.author_name, "Bob");
}

#[tokio::test]
async fn branch_names_lists_heads_sorted_and_skips_tags() {
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n")], "initial commit").await;
    git(work.path(), &["branch", "zeta"]).await;
    git(work.path(), &["branch", "alpha"]).await;
    git(work.path(), &["tag", "-a", "v1.0", "-m", "release"]).await;

    let names = branch_names(&work.git_dir()).await.unwrap();
    assert_eq!(names, vec!["alpha".to_string(), "main".to_string(), "zeta".to_string()]);
}
