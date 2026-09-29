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

// The cap has to stop the read, not just complain afterwards: an archive
// bigger than MAX_ARCHIVE_BYTES must never be fully buffered first. The
// fixture is a sparse 513 MiB file of zeros, streamed into a pack by
// `hash-object` (it's over git's default 512 MiB `core.bigFileThreshold`) so
// neither disk nor git ever holds it uncompressed; `archive` still reads
// 512 MiB of tar output before the cap trips.
#[tokio::test]
async fn archive_rejects_output_over_max_archive_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .env("GIT_DIR", &git_dir)
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "--quiet", "--bare", "--initial-branch=main"]);
    let big = dir.path().join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_ARCHIVE_BYTES + (1 << 20))
        .unwrap();
    let blob = git(&["hash-object", "-w", "--", big.to_str().unwrap()]);
    std::fs::remove_file(&big).unwrap();
    let tree = {
        let mut child = std::process::Command::new("git")
            .env("GIT_DIR", &git_dir)
            .arg("mktree")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        writeln!(child.stdin.take().unwrap(), "100644 blob {blob}\tbig.bin").unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let commit = git(&["commit-tree", "--no-gpg-sign", "-m", "big", &tree]);
    git(&["update-ref", "refs/heads/main", &commit]);

    let err = archive(&git_dir, "main", "tar").await.unwrap_err();
    assert!(err.to_string().contains("limit"), "unexpected error: {err}");
}
