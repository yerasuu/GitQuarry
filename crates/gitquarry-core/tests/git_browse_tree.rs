use gitquarry_core::git::browse::*;

#[tokio::test]
async fn search_filenames_matches_case_insensitively_across_subdirectories() {
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
    std::fs::create_dir(work.join("src")).unwrap();
    std::fs::write(work.join("src/Main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(work.join("README.md"), "hello\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "initial commit"]);

    let matches = search_filenames(&work.join(".git"), "main", "main").await.unwrap();
    assert_eq!(matches, vec!["src/Main.rs"]);
    assert!(
        search_filenames(&work.join(".git"), "main", "nope")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn language_stats_sums_bytes_per_language_and_skips_unknown_extensions() {
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
    std::fs::write(work.join("main.rs"), "aaaaaaaaaa").unwrap(); // 10 bytes
    std::fs::write(work.join("lib.rs"), "aaaaa").unwrap(); // 5 bytes
    std::fs::write(work.join("style.css"), "aaa").unwrap(); // 3 bytes
    std::fs::write(work.join("LICENSE"), "no extension, skipped").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "initial commit"]);

    let stats = language_stats(&work.join(".git"), "main").await.unwrap();
    assert_eq!(stats, vec![("Rust".to_string(), 15), ("CSS".to_string(), 3)]);
}
