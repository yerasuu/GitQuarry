//! `domain::repo::update_search_index` — builds/updates a repo's stored
//! `storage::search_index::SearchIndex` from real git history.

use gitquarry_core::app::AppContext;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::domain::repo::update_search_index;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::search_index::get_search_index;
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[43u8; SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        tracker: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn commit_file(work: &std::path::Path, filename: &str, content: &str, message: &str) {
    std::fs::write(work.join(filename), content).unwrap();
    run_git(work, &["add", "-A"]);
    run_git(work, &["commit", "-q", "-m", message]);
}

#[tokio::test]
async fn first_build_indexes_the_current_tree_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = tempfile::tempdir().unwrap();
    run_git(work.path(), &["init", "-q", "--initial-branch=main"]);
    commit_file(work.path(), "a.txt", "hello\n", "add a.txt");
    commit_file(work.path(), "b.txt", "world\n", "add b.txt");
    let git_dir = work.path().join(".git");

    update_search_index(&ctx, &uid, &git_dir).await;

    let (index, _etag) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    let index = index.unwrap();
    assert!(index.last_indexed_sha.is_some());
    assert!(index.files.contains(&"a.txt".to_string()));
    assert!(index.files.contains(&"b.txt".to_string()));
    assert_eq!(index.commits.len(), 2);
    // Newest-first.
    assert_eq!(index.commits[0].subject, "add b.txt");
    assert_eq!(index.commits[1].subject, "add a.txt");
}

#[tokio::test]
async fn a_later_push_appends_only_the_new_commits_and_refreshes_the_file_list() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = tempfile::tempdir().unwrap();
    run_git(work.path(), &["init", "-q", "--initial-branch=main"]);
    commit_file(work.path(), "a.txt", "hello\n", "add a.txt");
    let git_dir = work.path().join(".git");
    update_search_index(&ctx, &uid, &git_dir).await;

    commit_file(work.path(), "b.txt", "world\n", "add b.txt");
    update_search_index(&ctx, &uid, &git_dir).await;

    let (index, _etag) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    let index = index.unwrap();
    assert!(index.files.contains(&"a.txt".to_string()));
    assert!(index.files.contains(&"b.txt".to_string()));
    // Both commits present — the second call only had to walk the new one,
    // but the old one's already-stored entry is retained, not dropped.
    assert_eq!(index.commits.len(), 2);
    assert_eq!(index.commits[0].subject, "add b.txt");
    assert_eq!(index.commits[1].subject, "add a.txt");
}

#[tokio::test]
async fn re_indexing_at_the_same_head_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = tempfile::tempdir().unwrap();
    run_git(work.path(), &["init", "-q", "--initial-branch=main"]);
    commit_file(work.path(), "a.txt", "hello\n", "add a.txt");
    let git_dir = work.path().join(".git");
    update_search_index(&ctx, &uid, &git_dir).await;
    let (_index, etag_after_first) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();

    // No new commits since the last build — must not touch storage again.
    update_search_index(&ctx, &uid, &git_dir).await;
    let (_index, etag_after_second) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    assert_eq!(etag_after_first, etag_after_second);
}

#[tokio::test]
async fn empty_repo_is_left_unindexed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let work = tempfile::tempdir().unwrap();
    run_git(work.path(), &["init", "-q", "--initial-branch=main"]);
    let git_dir = work.path().join(".git");

    update_search_index(&ctx, &uid, &git_dir).await;

    let (index, _etag) = get_search_index(ctx.backend().as_ref(), &ctx.cipher, &uid)
        .await
        .unwrap();
    assert!(index.is_none());
}
