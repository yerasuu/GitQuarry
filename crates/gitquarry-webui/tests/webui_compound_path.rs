use gitquarry_webui::webui::compound_path::*;
use std::path::Path;
use std::process::Command;

fn run(dir: &Path, args: &[&str]) {
    let status = Command::new("git").current_dir(dir).args(args).status().unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A bare repo with one commit (`f.txt`) and the given extra branches
/// created off it (`git branch` on top of the initial commit).
fn bare_repo_with_branches(dir: &Path, branches: &[&str]) -> std::path::PathBuf {
    let work = dir.join("work");
    std::fs::create_dir(&work).unwrap();
    run(&work, &["init", "--quiet", "-b", "main"]);
    run(&work, &["config", "user.email", "a@b.c"]);
    run(&work, &["config", "user.name", "Test"]);
    std::fs::write(work.join("f.txt"), "hi").unwrap();
    run(&work, &["add", "."]);
    run(&work, &["commit", "--quiet", "-m", "init"]);
    for branch in branches {
        run(&work, &["branch", branch]);
    }
    let bare = dir.join("bare.git");
    run(
        dir,
        &[
            "clone",
            "--quiet",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    bare
}

#[tokio::test]
async fn splits_branch_and_path_by_checking_real_refs() {
    let dir = tempfile::tempdir().unwrap();

    // Branch "release" exists (not "release/v1" — git's ref namespace
    // can't hold both at once, same as it can't hold both a file and a
    // directory at one path): "release/v1/f.txt" must split as
    // branch="release", path="v1/f.txt".
    let bare = bare_repo_with_branches(dir.path(), &["release"]);
    let (branch, path) = split_branch_and_path(&bare, &["release", "v1", "f.txt"]).await.unwrap();
    assert_eq!(branch, "release");
    assert_eq!(path, "v1/f.txt");
}

#[tokio::test]
async fn falls_back_to_the_longer_ref_when_the_short_prefix_is_not_one() {
    let dir = tempfile::tempdir().unwrap();

    // This time only "release/v1" exists as a branch (no bare
    // "release"), so the same input segments must split the other way:
    // branch="release/v1", path="f.txt".
    let bare = bare_repo_with_branches(dir.path(), &["release/v1"]);
    let (branch, path) = split_branch_and_path(&bare, &["release", "v1", "f.txt"]).await.unwrap();
    assert_eq!(branch, "release/v1");
    assert_eq!(path, "f.txt");
}

#[test]
fn find_marker_locates_a_multi_segment_marker() {
    let segments = ["team", "proj", "refs", "heads", "main", "f.txt"];
    assert_eq!(find_marker(&segments, &["refs", "heads"]), Some(2));
    assert_eq!(find_marker(&segments, &["blob"]), None);
}
