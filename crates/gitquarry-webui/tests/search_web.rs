use gitquarry_core::app::AppContext;
use gitquarry_webui::search::web::*;

#[test]
fn render_search_page_echoes_the_query_without_results() {
    let html = render_search_page("hello", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"input name="q" value="hello""#));
    assert!(!html.contains("<h4>"));
}

#[test]
fn render_search_page_shows_hits_and_truncation_notice() {
    let results = SearchResults {
        file_hits: vec![filenames::FilenameHit {
            repo: "acme/web".to_string(),
            rev: "main".to_string(),
            path: "src/main.rs".to_string(),
        }],
        files_truncated: true,
        commit_hits: vec![commits::CommitHit {
            repo: "acme/web".to_string(),
            sha: "abc123".to_string(),
            subject: "fix bug".to_string(),
        }],
        commits_truncated: false,
    };
    let html = render_search_page("main", Some(results), gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("src/main.rs"));
    assert!(html.contains("fix bug"));
    assert!(html.contains("showing the first 100 matches"));
}

fn test_ctx(root: &std::path::Path) -> AppContext {
    use gitquarry_core::app::Locks;
    use gitquarry_core::config::{LocalEngine, LocalParams};
    use gitquarry_core::crypto::Cipher;
    use gitquarry_core::storage::Backend;
    use gitquarry_core::storage::local::LocalBackend;
    use std::sync::Arc;
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = Arc::new(Cipher::derive("test passphrase", &[19u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend: Arc::new(backend) as Arc<dyn Backend>,
        locks: Locks::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// Proves the fast path really reads the stored index rather than falling
/// back to live `git` — the repo below has no local mirror/working tree
/// on disk at all (`create_repo` only claims the name+uid and writes an
/// empty manifest), so a live `git ls-tree`/`git log` against it would
/// fail; the only way this can find a hit is by reading
/// `storage::search_index::SearchIndex` directly.
#[tokio::test]
async fn filename_search_reads_the_stored_index_instead_of_live_git() {
    use gitquarry_core::storage::search_index::{IndexedCommit, SearchIndex, put_search_index};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    let resolved = ctx.create_repo("acme/demo").await.unwrap().unwrap();
    let index = SearchIndex {
        last_indexed_sha: Some("planted-sha".to_string()),
        files: vec!["planted.txt".to_string()],
        commits: vec![IndexedCommit {
            sha: "planted-sha".to_string(),
            subject: "planted subject".to_string(),
        }],
    };
    put_search_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, &index, None)
        .await
        .unwrap();

    let (file_hits, _truncated) = filenames::search_all(&ctx, &["acme/demo".to_string()], "planted").await;
    assert_eq!(file_hits.len(), 1);
    assert_eq!(file_hits[0].path, "planted.txt");
    assert_eq!(file_hits[0].rev, "planted-sha");

    let (commit_hits, _truncated) = commits::search_all(&ctx, &["acme/demo".to_string()], "planted").await;
    assert_eq!(commit_hits.len(), 1);
    assert_eq!(commit_hits[0].subject, "planted subject");
}

/// A repo that resolves locally but has never been indexed (no
/// `SearchIndex` stored yet) falls back to the live path instead of
/// silently contributing zero hits forever.
#[tokio::test]
async fn filename_search_falls_back_to_live_git_without_a_stored_index() {
    use gitquarry_core::storage::entities::{RepoConfig, Visibility};
    use std::sync::Arc;
    use tokio::process::Command;

    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    let resolved = ctx.create_repo("acme/demo").await.unwrap().unwrap();
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/demo".to_string(),
        visibility: Visibility::Public,
        ..Default::default()
    };
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();

    let work = tempfile::tempdir().unwrap();
    async fn run_git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git").current_dir(dir).args(args).status().await.unwrap();
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }
    run_git(work.path(), &["init", "-q"]).await;
    std::fs::write(work.path().join("live.txt"), b"live content\n").unwrap();
    run_git(work.path(), &["add", "-A"]).await;
    run_git(
        work.path(),
        &[
            "-c",
            "user.email=test@test",
            "-c",
            "user.name=test",
            "commit",
            "-q",
            "-m",
            "add live file",
        ],
    )
    .await;
    let sha_out = Command::new("git")
        .current_dir(work.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .unwrap();
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
    let git_dir = work.path().join(".git");
    let pack_dir = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(&git_dir, &sha, &[], pack_dir.path())
        .await
        .unwrap()
        .unwrap();
    resolved
        .remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    resolved
        .remote
        .put_manifest(
            &gitquarry_core::storage::remote::Manifest {
                refs: std::collections::BTreeMap::from([("refs/heads/main".to_string(), sha)]),
                head: Some("refs/heads/main".to_string()),
            },
            None,
        )
        .await
        .unwrap();

    let (file_hits, _truncated) = filenames::search_all(&ctx, &["acme/demo".to_string()], "live").await;
    assert_eq!(file_hits.len(), 1);
    assert_eq!(file_hits[0].path, "live.txt");
}
