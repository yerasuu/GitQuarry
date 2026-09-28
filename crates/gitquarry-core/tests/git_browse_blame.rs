use gitquarry_core::git::browse::blame;
use std::path::Path;
use std::process::Command;

fn git(work: &Path, author: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(work)
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

// Two lines from the first commit exercise porcelain's "repeated commit
// omits its metadata block" case: the second line must still inherit Jane
// as its author.
#[tokio::test]
async fn blame_attributes_each_line_to_its_commit_and_author() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git(work, "Jane Doe", &["init", "--quiet", "--initial-branch=main"]);
    std::fs::create_dir(work.join("src")).unwrap();
    std::fs::write(work.join("src/x.rs"), "fn main() {\n}\n").unwrap();
    git(work, "Jane Doe", &["add", "."]);
    git(
        work,
        "Jane Doe",
        &["commit", "--quiet", "--no-gpg-sign", "-m", "first commit"],
    );
    let first = git(work, "Jane Doe", &["rev-parse", "HEAD"]);
    std::fs::write(work.join("src/x.rs"), "fn main() {\n}\n// tail\n").unwrap();
    git(
        work,
        "John Roe",
        &["commit", "--quiet", "--no-gpg-sign", "-am", "second commit"],
    );
    let second = git(work, "John Roe", &["rev-parse", "HEAD"]);

    let lines = blame(&work.join(".git"), "main", "src/x.rs").await.unwrap();

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
