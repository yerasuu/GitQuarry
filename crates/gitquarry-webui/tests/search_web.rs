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
        peer_metadata_hits: vec![PeerMetadataHit {
            peer: "instance-b".to_string(),
            name: "acme/other".to_string(),
            description: Some("a peer's repo".to_string()),
        }],
    };
    let html = render_search_page("main", Some(results), gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("src/main.rs"));
    assert!(html.contains("fix bug"));
    assert!(html.contains("showing the first 100 matches"));
    assert!(html.contains(r#"href="/_remote/instance-b/acme/other""#));
}

#[test]
fn display_repo_labels_a_peer_mirror_as_remote() {
    assert_eq!(display_repo("acme/web"), "acme/web");
    assert_eq!(
        display_repo("_remote/instance-b/acme/demo"),
        "instance-b/acme/demo (remote)"
    );
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
        federation: None,
        allow_push_create: false,
        events: Default::default(),
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

/// The core federation-layer-3 invariant: a repo candidate list built
/// from disk never causes a search to reach out to a peer over the
/// network. No server is even running in this test — if `mirror_and_head`
/// tried to sync, it would hang or error against a closed port instead
/// of returning cleanly.
#[tokio::test]
async fn mirror_and_head_never_syncs_a_peer_repo_that_isnt_already_mirrored() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    assert!(mirror_and_head(&ctx, "_remote/instance-b/acme/demo").await.is_none());
    assert!(list_synced_peer_repos(&ctx).await.is_empty());
}

/// End-to-end, real socket: a peer repo shows up in filename/commit
/// content search once its mirror has been synced (`peer_repo_metadata`
/// itself is federation-backed now — see [`search_peer_metadata`]'s own
/// doc comment — and covered at that boundary, not here).
#[tokio::test]
async fn peer_repo_shows_up_in_content_search_once_its_mirror_is_synced() {
    use gitquarry_core::storage::entities::Visibility;
    use std::sync::Arc;
    use tokio::process::Command;

    let dir_b = tempfile::tempdir().unwrap();
    let ctx_b = Arc::new(test_ctx(dir_b.path()));
    let resolved = ctx_b.create_repo("acme/demo").await.unwrap().unwrap();
    let config = gitquarry_core::storage::entities::RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/demo".to_string(),
        visibility: Visibility::Public,
        ..Default::default()
    };
    gitquarry_core::storage::settings::put_repo_config(
        ctx_b.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx_b.cipher,
    )
    .await
    .unwrap();
    gitquarry_core::storage::settings::set_visible_index(ctx_b.backend().as_ref(), &ctx_b.cipher, &resolved.uid, true)
        .await;

    let work = tempfile::tempdir().unwrap();
    async fn run_git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git").current_dir(dir).args(args).status().await.unwrap();
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }
    run_git(work.path(), &["init", "-q"]).await;
    std::fs::write(work.path().join("needle.txt"), b"unique marker line\n").unwrap();
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
            "add needle",
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

    let bind_addr = "127.0.0.1:18457";
    tokio::spawn(gitquarry_webui::webui::run_server(bind_addr, Arc::clone(&ctx_b)));
    let probe = reqwest::Client::new();
    for _ in 0..100 {
        if probe
            .get(format!("http://{bind_addr}/_api/instance"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let dir_a = tempfile::tempdir().unwrap();
    let ctx_a = Arc::new(test_ctx(dir_a.path()));

    assert!(list_synced_peer_repos(&ctx_a).await.is_empty());

    ctx_a
        .open_remote_mirror_at("instance-b", &format!("http://{bind_addr}"), "acme/demo")
        .await
        .unwrap();

    let synced = list_synced_peer_repos(&ctx_a).await;
    assert_eq!(synced, vec!["_remote/instance-b/acme/demo".to_string()]);

    let (file_hits, _truncated) = filenames::search_all(&ctx_a, &synced, "needle").await;
    assert_eq!(file_hits.len(), 1);
    assert_eq!(file_hits[0].repo, "_remote/instance-b/acme/demo");
    assert_eq!(file_hits[0].path, "needle.txt");

    let (commit_hits, _truncated) = commits::search_all(&ctx_a, &synced, "needle").await;
    assert_eq!(commit_hits.len(), 1);
}
