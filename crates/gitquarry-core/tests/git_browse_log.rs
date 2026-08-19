use gitquarry_core::git::browse::*;

#[test]
fn parses_log_line_with_decoration() {
    let line =
        "abc123\x1fJane Doe\x1fjane@example.com\x1f2026-01-02T10:00:00+00:00\x1ffix bug\x1fHEAD -> main, tag: v1.0";
    let c = parse_log_line(line).unwrap();
    assert_eq!(c.sha, "abc123");
    assert_eq!(c.author_name, "Jane Doe");
    assert_eq!(c.subject, "fix bug");
    assert_eq!(c.decoration, "HEAD -> main, tag: v1.0");
}

#[test]
fn parses_log_line_without_decoration() {
    let line = "abc123\x1fJane\x1fjane@x.com\x1f2026-01-02\x1fsubject";
    let c = parse_log_line(line).unwrap();
    assert_eq!(c.subject, "subject");
    assert_eq!(c.decoration, "");
}

#[test]
fn parses_shortlog_counts() {
    let text = "    12\tJane Doe\n     3\tJohn Roe\n";
    let counts = parse_shortlog(text);
    assert_eq!(counts, vec![(12, "Jane Doe".to_string()), (3, "John Roe".to_string())]);
}

fn init_repo() -> tempfile::TempDir {
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
    dir
}

#[tokio::test]
async fn log_page_with_stats_reports_a_multi_file_change() {
    let dir = init_repo();
    let work = dir.path();
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    std::fs::write(work.join("b.txt"), "two\nthree\n").unwrap();
    std::process::Command::new("git")
        .current_dir(work)
        .args(["add", "."])
        .status()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(work)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(["commit", "--quiet", "-m", "add two files"])
        .status()
        .unwrap();

    let rows = log_page_with_stats(&work.join(".git"), "HEAD", 10, 0, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let (commit, stats) = &rows[0];
    assert_eq!(commit.subject, "add two files");
    let stats = stats.unwrap();
    assert_eq!(stats.files_changed, 2);
    assert_eq!(stats.insertions, 3);
    assert_eq!(stats.deletions, 0);
}

#[tokio::test]
async fn log_page_with_stats_handles_a_rename_only_commit() {
    let dir = init_repo();
    let work = dir.path();
    let commit = |args: &[&str]| {
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
    std::fs::write(work.join("old.txt"), "content\n").unwrap();
    commit(&["add", "."]);
    commit(&["commit", "--quiet", "-m", "add file"]);
    std::fs::rename(work.join("old.txt"), work.join("new.txt")).unwrap();
    commit(&["add", "-A"]);
    commit(&["commit", "--quiet", "-m", "rename file"]);

    let rows = log_page_with_stats(&work.join(".git"), "HEAD", 1, 0, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let (commit_info, stats) = &rows[0];
    assert_eq!(commit_info.subject, "rename file");
    // A pure rename has no content change -- `--shortstat` may omit the
    // stat line for it entirely (nothing to report), which is exactly what
    // this asserts against: `stats` staying `None` shouldn't panic or
    // misattach a later/earlier commit's numbers.
    assert!(
        stats.is_none()
            || *stats
                == Some(CommitStats {
                    files_changed: 1,
                    insertions: 0,
                    deletions: 0
                })
    );
}

#[tokio::test]
async fn search_log_with_stats_matches_the_grep_and_still_reports_stats() {
    let dir = init_repo();
    let work = dir.path();
    let commit = |args: &[&str]| {
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
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    commit(&["add", "."]);
    commit(&["commit", "--quiet", "-m", "unrelated change"]);
    std::fs::write(work.join("a.txt"), "one\ntwo\n").unwrap();
    commit(&["add", "."]);
    commit(&["commit", "--quiet", "-m", "fix the bug"]);

    let rows = search_log_with_stats(&work.join(".git"), "HEAD", "bug", 10, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let (commit_info, stats) = &rows[0];
    assert_eq!(commit_info.subject, "fix the bug");
    assert_eq!(stats.unwrap().insertions, 1);
}
