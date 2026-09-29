// `archive` itself is a private submodule of `git::browse` (see that
// module's doc comment); its public items are re-exported at
// `git::browse::*`, which is the path visible from outside the crate.
use gitquarry_core::git::browse::*;
use gitquarry_test_support::{Work, git};
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
    let work = Work::new().await;
    work.commit(
        &[("sub/nested.txt", "hello from a subdir\n"), ("top.txt", "top level\n")],
        "c",
    )
    .await;
    let git_dir = work.git_dir();

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
    let work = Work::new().await;
    work.commit(&[("f.txt", "content\n")], "c").await;
    let git_dir = work.git_dir();

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
    let work = Work::new().await;
    let big = dir.path().join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_ARCHIVE_BYTES + (1 << 20))
        .unwrap();
    let blob = git(work.path(), &["hash-object", "-w", "--", big.to_str().unwrap()]).await;
    std::fs::remove_file(&big).unwrap();
    // Staged straight from the object store, so the blob is never checked
    // out (or rehashed) at full size.
    let cacheinfo = format!("100644,{blob},big.bin");
    git(work.path(), &["update-index", "--add", "--cacheinfo", &cacheinfo]).await;
    let tree = git(work.path(), &["write-tree"]).await;
    let commit = git(work.path(), &["commit-tree", "-m", "big", &tree]).await;
    git(work.path(), &["update-ref", "refs/heads/main", &commit]).await;
    let git_dir = work.git_dir();

    let err = archive(&git_dir, "main", "tar").await.unwrap_err();
    assert!(err.to_string().contains("limit"), "unexpected error: {err}");
}
