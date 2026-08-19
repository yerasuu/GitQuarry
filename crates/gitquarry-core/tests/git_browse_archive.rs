// `archive` itself is a private submodule of `git::browse` (see that
// module's doc comment); its public items are re-exported at
// `git::browse::*`, which is the path visible from outside the crate.
use gitquarry_core::git::browse::*;
use std::io::Read;

#[test]
fn archive_filename_uses_the_repos_last_path_segment() {
    assert_eq!(archive_filename("team/proj", "v1.0", "tar.gz"), "proj-v1.0.tar.gz");
    assert_eq!(archive_filename("proj", "main", "zip"), "proj-main.zip");
}

#[test]
fn archive_filename_flattens_a_slash_in_the_rev() {
    assert_eq!(archive_filename("proj", "feature/x", "zip"), "proj-feature-x.zip");
}

#[tokio::test]
async fn archive_pair_zip_matches_a_native_git_archive_zip() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    let git_dir = work.join(".git");
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
    std::fs::create_dir(work.join("sub")).unwrap();
    std::fs::write(work.join("sub/nested.txt"), "hello from a subdir\n").unwrap();
    std::fs::write(work.join("top.txt"), "top level\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "c"]);

    let (tar_gz, zip_bytes) = archive_pair(&git_dir, "main").await.unwrap();
    assert!(!tar_gz.is_empty());
    assert!(!zip_bytes.is_empty());

    // tar.gz: a real gzip stream that decompresses back to a valid tar
    // with the same two files `git archive --format=tar` would produce.
    let mut gunzipped = Vec::new();
    flate2::read::GzDecoder::new(&tar_gz[..])
        .read_to_end(&mut gunzipped)
        .unwrap();
    let mut names: Vec<String> = tar::Archive::new(&gunzipped[..])
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(names.iter().any(|n| n.ends_with("top.txt")), "{names:?}");
    assert!(names.iter().any(|n| n.ends_with("sub/nested.txt")), "{names:?}");

    // zip: same two files, same content — re-derived in-process from the
    // tar, not from a second git decode, but must still be a correct zip.
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).unwrap();
    let mut top = String::new();
    zip.by_name("top.txt").unwrap().read_to_string(&mut top).unwrap();
    assert_eq!(top, "top level\n");
    let mut nested = String::new();
    zip.by_name("sub/nested.txt")
        .unwrap()
        .read_to_string(&mut nested)
        .unwrap();
    assert_eq!(nested, "hello from a subdir\n");
}

#[tokio::test]
async fn archive_one_matches_archive_pair_for_each_format() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    let git_dir = work.join(".git");
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
    std::fs::write(work.join("f.txt"), "content\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "c"]);

    // Same mechanism, so a normal branch's on-demand download
    // (`archive_one`, one format at a time) and a release's generated
    // pair (`archive_pair`, both at once) never quietly diverge.
    let (pair_tar_gz, pair_zip) = archive_pair(&git_dir, "main").await.unwrap();
    let one_tar_gz = archive_one(&git_dir, "main", "tar.gz").await.unwrap();
    let one_zip = archive_one(&git_dir, "main", "zip").await.unwrap();

    let mut pair_gunzipped = Vec::new();
    flate2::read::GzDecoder::new(&pair_tar_gz[..])
        .read_to_end(&mut pair_gunzipped)
        .unwrap();
    let mut one_gunzipped = Vec::new();
    flate2::read::GzDecoder::new(&one_tar_gz[..])
        .read_to_end(&mut one_gunzipped)
        .unwrap();
    assert_eq!(pair_gunzipped, one_gunzipped);

    let mut pair_zip = zip::ZipArchive::new(std::io::Cursor::new(pair_zip)).unwrap();
    let mut one_zip = zip::ZipArchive::new(std::io::Cursor::new(one_zip)).unwrap();
    let mut pair_content = String::new();
    pair_zip
        .by_name("f.txt")
        .unwrap()
        .read_to_string(&mut pair_content)
        .unwrap();
    let mut one_content = String::new();
    one_zip
        .by_name("f.txt")
        .unwrap()
        .read_to_string(&mut one_content)
        .unwrap();
    assert_eq!(pair_content, "content\n");
    assert_eq!(pair_content, one_content);

    assert!(archive_one(&git_dir, "main", "rar").await.is_err());
}

/// The cap has to stop the read, not just complain afterwards — an archive
/// bigger than the ceiling must never be fully buffered first. Exercised
/// through `run_capped` directly (an actual 512 MiB tree is not a test
/// fixture); `git::browse::archive` is the one caller, always with
/// `MAX_ARCHIVE_BYTES`.
#[tokio::test]
async fn run_capped_rejects_output_over_the_ceiling() {
    use std::ffi::OsStr;

    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    let git_dir = work.join(".git");
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
    std::fs::write(work.join("top.txt"), "top level\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "c"]);

    // The same archive that succeeds uncapped fails under a 1-byte ceiling.
    let uncapped = gitquarry_core::git::run_capped(
        Some(&git_dir),
        &[
            OsStr::new("archive"),
            OsStr::new("--format=tar"),
            OsStr::new("--end-of-options"),
            OsStr::new("main"),
        ],
        1 << 20,
    )
    .await
    .unwrap();
    assert!(!uncapped.is_empty());

    let err = gitquarry_core::git::run_capped(
        Some(&git_dir),
        &[
            OsStr::new("archive"),
            OsStr::new("--format=tar"),
            OsStr::new("--end-of-options"),
            OsStr::new("main"),
        ],
        1,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("limit"), "unexpected error: {err}");
}
