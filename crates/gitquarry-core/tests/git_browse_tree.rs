use gitquarry_core::git::browse::*;
use gitquarry_test_support::Work;

#[tokio::test]
async fn search_filenames_matches_case_insensitively_across_subdirectories() {
    let work = Work::new().await;
    work.commit(
        &[("src/Main.rs", "fn main() {}\n"), ("README.md", "hello\n")],
        "initial commit",
    )
    .await;

    let matches = search_filenames(&work.git_dir(), "main", "main").await.unwrap();
    assert_eq!(matches, vec!["src/Main.rs"]);
    assert!(
        search_filenames(&work.git_dir(), "main", "nope")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn language_stats_sums_bytes_per_language_and_skips_unknown_extensions() {
    let work = Work::new().await;
    work.commit(
        &[
            ("main.rs", "aaaaaaaaaa"), // 10 bytes
            ("lib.rs", "aaaaa"),       // 5 bytes
            ("style.css", "aaa"),      // 3 bytes
            ("LICENSE", "no extension, skipped"),
        ],
        "initial commit",
    )
    .await;

    let stats = language_stats(&work.git_dir(), "main").await.unwrap();
    assert_eq!(stats, vec![("Rust".to_string(), 15), ("CSS".to_string(), 3)]);
}
