use gitquarry_core::git::browse::blame;
use gitquarry_test_support::{Work, git};

// Two lines from the first commit exercise porcelain's "repeated commit
// omits its metadata block" case: the second line must still inherit Jane
// as its author.
#[tokio::test]
async fn blame_attributes_each_line_to_its_commit_and_author() {
    let work = Work::new().await;
    std::fs::create_dir(work.path().join("src")).unwrap();
    std::fs::write(work.path().join("src/x.rs"), "fn main() {\n}\n").unwrap();
    git(work.path(), &["add", "."]).await;
    git(
        work.path(),
        &[
            "commit",
            "--quiet",
            "--author=Jane Doe <author@example.com>",
            "-m",
            "first commit",
        ],
    )
    .await;
    let first = git(work.path(), &["rev-parse", "HEAD"]).await;
    std::fs::write(work.path().join("src/x.rs"), "fn main() {\n}\n// tail\n").unwrap();
    git(
        work.path(),
        &[
            "commit",
            "--quiet",
            "--author=John Roe <author@example.com>",
            "-am",
            "second commit",
        ],
    )
    .await;
    let second = git(work.path(), &["rev-parse", "HEAD"]).await;

    let lines = blame(&work.git_dir(), "main", "src/x.rs").await.unwrap();

    assert_eq!(lines.len(), 3);
    assert_eq!(
        (lines[0].sha.as_str(), lines[0].author.as_str()),
        (first.as_str(), "Jane Doe")
    );
    assert_eq!(lines[0].content, "fn main() {");
    assert_eq!(
        (lines[1].sha.as_str(), lines[1].author.as_str()),
        (first.as_str(), "Jane Doe")
    );
    assert_eq!(lines[1].content, "}");
    assert_eq!(
        (lines[2].sha.as_str(), lines[2].author.as_str()),
        (second.as_str(), "John Roe")
    );
    assert_eq!(lines[2].content, "// tail");
}
