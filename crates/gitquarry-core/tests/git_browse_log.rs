use gitquarry_core::git::browse::*;
use gitquarry_test_support::Work;

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

#[tokio::test]
async fn log_page_with_stats_reports_a_multi_file_change() {
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n"), ("b.txt", "two\nthree\n")], "add two files")
        .await;

    let rows = log_page_with_stats(&work.git_dir(), "HEAD", 10, 0, None).await.unwrap();
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
    let work = Work::new().await;
    work.commit(&[("old.txt", "content\n")], "add file").await;
    std::fs::rename(work.path().join("old.txt"), work.path().join("new.txt")).unwrap();
    work.commit(&[], "rename file").await;

    let rows = log_page_with_stats(&work.git_dir(), "HEAD", 1, 0, None).await.unwrap();
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
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n")], "unrelated change").await;
    work.commit(&[("a.txt", "one\ntwo\n")], "fix the bug").await;

    let rows = search_log_with_stats(&work.git_dir(), "HEAD", "bug", 10, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let (commit_info, stats) = &rows[0];
    assert_eq!(commit_info.subject, "fix the bug");
    assert_eq!(stats.unwrap().insertions, 1);
}
