use gitquarry_core::git::{list_refs, update_ref};
use gitquarry_mirror_external::git_remote::*;
use gitquarry_test_support::git;
use std::path::Path;

fn capture_git(git_dir: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .env("GIT_DIR", git_dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// One commit whose tree is exactly `files` (name -> content).
fn commit_with_files(git_dir: &Path, parent: Option<&str>, files: &[(&str, &str)], message: &str) -> String {
    let mut tree_input = String::new();
    for (name, content) in files {
        let blob = capture_git(git_dir, &["hash-object", "-w", "--stdin"], content);
        tree_input.push_str(&format!("100644 blob {blob}\t{name}\n"));
    }
    let tree = capture_git(git_dir, &["mktree"], &tree_input);
    let mut args = vec!["commit-tree", tree.as_str()];
    if let Some(p) = parent {
        args.push("-p");
        args.push(p);
    }
    args.push("-m");
    args.push(message);
    capture_git(git_dir, &args, "")
}

#[tokio::test]
async fn ls_remote_refs_lists_every_ref_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let sha = commit_with_files(&git_dir, None, &[("f.txt", "hello\n")], "init");
    update_ref(&git_dir, "refs/heads/main", &sha).await.unwrap();

    let refs = ls_remote_refs(&git_dir.to_string_lossy(), "").await.unwrap();
    assert_eq!(refs.get("refs/heads/main"), Some(&sha));
}

#[tokio::test]
async fn fetch_ref_from_remote_url_brings_objects_without_moving_local_refs() {
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_git_dir = remote_dir.path().join("remote.git");
    std::fs::create_dir_all(&remote_git_dir).unwrap();
    git(&remote_git_dir, &["init", "--quiet", "--bare"]).await;
    let sha = commit_with_files(&remote_git_dir, None, &[("f.txt", "hello\n")], "init");
    update_ref(&remote_git_dir, "refs/heads/main", &sha).await.unwrap();

    let local_dir = tempfile::tempdir().unwrap();
    let local_git_dir = local_dir.path().join("local.git");
    std::fs::create_dir_all(&local_git_dir).unwrap();
    git(&local_git_dir, &["init", "--quiet", "--bare"]).await;

    fetch_ref_from_remote_url(&local_git_dir, &remote_git_dir.to_string_lossy(), "", "refs/heads/main")
        .await
        .unwrap();
    // Objects arrived, but no local ref was moved by the fetch itself.
    assert!(list_refs(&local_git_dir).await.unwrap().is_empty());

    update_ref(&local_git_dir, "refs/heads/main", &sha).await.unwrap();
    let refs = list_refs_with_sha(&local_git_dir).await.unwrap();
    assert_eq!(refs.get("refs/heads/main"), Some(&sha));
}

#[tokio::test]
async fn list_refs_with_sha_maps_every_ref_to_its_object() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let sha = commit_with_files(&git_dir, None, &[("f.txt", "hello\n")], "init");
    update_ref(&git_dir, "refs/heads/main", &sha).await.unwrap();
    update_ref(&git_dir, "refs/tags/v1", &sha).await.unwrap();

    let refs = list_refs_with_sha(&git_dir).await.unwrap();
    assert_eq!(refs.get("refs/heads/main"), Some(&sha));
    assert_eq!(refs.get("refs/tags/v1"), Some(&sha));
}

#[test]
fn redact_url_strips_embedded_credentials() {
    let text = "fatal: could not push to https://x-access-token:ghp_secret@github.com/a/b.git";
    let redacted = redact_url(text);
    assert!(!redacted.contains("ghp_secret"));
    assert!(redacted.contains("https://github.com/a/b.git"));
}

#[test]
fn redact_url_strips_every_occurrence() {
    let text = "https://tok:s3cr3t@host/a first, https://tok:s3cr3t@host/a again";
    let redacted = redact_url(text);
    assert!(!redacted.contains("s3cr3t"));
}

#[tokio::test]
async fn ls_remote_refs_skips_the_peeled_entry_of_an_annotated_tag() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "--quiet", "--bare"]).await;
    let sha = commit_with_files(&git_dir, None, &[("f.txt", "hello\n")], "init");
    update_ref(&git_dir, "refs/heads/main", &sha).await.unwrap();
    git(&git_dir, &["tag", "-a", "v1", "-m", "release", &sha]).await;

    let refs = ls_remote_refs(&git_dir.to_string_lossy(), "").await.unwrap();
    // The tag itself is a real ref (pointing at the tag object, not the
    // commit); its `^{}` companion is not a name any refspec can carry.
    assert!(refs.contains_key("refs/tags/v1"));
    assert_ne!(refs.get("refs/tags/v1"), Some(&sha));
    assert!(
        refs.keys().all(|name| !name.ends_with("^{}")),
        "peeled entries leaked into the ref map: {:?}",
        refs.keys().collect::<Vec<_>>()
    );
}
