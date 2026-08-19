// `refs` is a private submodule of `git::browse`, re-exported via `pub use
// refs::*;` there, so the browsing module itself is the visible path.
use gitquarry_core::git::browse::*;

#[test]
fn parses_ref_line_for_branch_and_annotated_tag() {
    let branch = "refs/heads/main\x1fabc\x1flatest commit\x1f2026-01-02T10:00:00+00:00\x1fAlice";
    let r = parse_ref_line(branch).unwrap();
    assert_eq!(r.name, "refs/heads/main");
    assert_eq!(r.subject, "latest commit");
    assert_eq!(r.author_name, "Alice");

    let tag = "refs/tags/v1.0\x1fdef\x1frelease notes\x1f2026-02-01T00:00:00+00:00\x1fBob";
    let r = parse_ref_line(tag).unwrap();
    assert_eq!(r.subject, "release notes");
    assert_eq!(r.author_name, "Bob");
}

#[test]
fn parses_ref_line_without_an_author_field() {
    // Whatever git version is on PATH, `%(if)%(taggername)%(then)...%(end)`
    // always resolves to *something* (empty string included) — this just
    // guards the parser itself doesn't panic if the field is ever missing.
    let branch = "refs/heads/main\x1fabc\x1flatest commit\x1f2026-01-02T10:00:00+00:00";
    let r = parse_ref_line(branch).unwrap();
    assert_eq!(r.author_name, "");
}

#[tokio::test]
async fn list_refs_detailed_credits_the_tagger_not_the_committer_for_an_annotated_tag() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    let run = |args: &[&str], author: &str| {
        let status = std::process::Command::new("git")
            .current_dir(work)
            .env("GIT_AUTHOR_NAME", author)
            .env("GIT_AUTHOR_EMAIL", "author@example.com")
            .env("GIT_COMMITTER_NAME", author)
            .env("GIT_COMMITTER_EMAIL", "author@example.com")
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "--quiet", "--initial-branch=main"], "Alice");
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    run(&["add", "."], "Alice");
    run(&["commit", "--quiet", "-m", "initial commit"], "Alice");
    // `run`'s own `GIT_COMMITTER_NAME`/`GIT_AUTHOR_NAME` env vars are what
    // actually set the tagger identity (env always wins over `-c`), so
    // tagging as "Bob" here is what makes this test's commit author
    // ("Alice") and tag tagger ("Bob") genuinely different identities.
    run(&["tag", "-a", "v1.0", "-m", "release"], "Bob");

    let refs = list_refs_detailed(&work.join(".git"), "refs/").await.unwrap();
    let branch = refs.iter().find(|r| r.name == "refs/heads/main").unwrap();
    assert_eq!(branch.author_name, "Alice");
    let tag = refs.iter().find(|r| r.name == "refs/tags/v1.0").unwrap();
    assert_eq!(tag.author_name, "Bob");
}

#[tokio::test]
async fn branch_names_lists_heads_sorted_and_skips_tags() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
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
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "initial commit"]);
    run(&["branch", "zeta"]);
    run(&["branch", "alpha"]);
    run(&["tag", "-a", "v1.0", "-m", "release"]);

    let names = branch_names(&work.join(".git")).await.unwrap();
    assert_eq!(names, vec!["alpha".to_string(), "main".to_string(), "zeta".to_string()]);
}
