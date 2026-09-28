use gitquarry_core::mirror::*;

#[test]
fn quotes_paths_with_spaces_and_quotes() {
    assert_eq!(shell_quote("/usr/bin/x"), "'/usr/bin/x'");
    assert_eq!(shell_quote("/a b/x"), "'/a b/x'");
    assert_eq!(shell_quote("/a'b"), r"'/a'\''b'");
}

/// End-to-end: seeds a real repo on "instance B" (a minimal axum server
/// running just [`crate::git::http::middleware`] — no `gitquarry-webui`
/// dependency needed here, this only exercises git-smart-HTTP serving
/// from a real local-filesystem-backed `Remote`), then has "instance A"
/// fetch it via `AppContext::open_remote_mirror` against B's real HTTP
/// URL — the same mechanism `webui::repo::handler_remote_repo` (in
/// `gitquarry-webui`) uses, exercised over a genuine git-smart-HTTP
/// round trip. Mirrors `federation::tests`' own "spin up a real peer"
/// pattern used for layer 1.
#[tokio::test]
async fn open_remote_mirror_fetches_a_real_peers_repo_over_git_smart_http() {
    use gitquarry_core::app::{AppContext, Locks};
    use gitquarry_core::config::{LocalEngine, LocalParams};
    use gitquarry_core::crypto::Cipher;
    use gitquarry_core::storage::Backend;
    use gitquarry_core::storage::entities::{RepoConfig, Visibility};
    use gitquarry_core::storage::remote::Manifest;
    use gitquarry_core::storage::settings;
    use s3db::LocalBackend;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::process::Command;

    fn test_ctx(root: &std::path::Path) -> AppContext {
        let backend = LocalBackend::new(&LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: LocalEngine::Files,
        });
        let cipher = Arc::new(Cipher::derive("test passphrase", &[9u8; s3db_crypto::SALT_LEN]).unwrap());
        AppContext {
            bucket: "test".to_string(),
            ssh_bind: "127.0.0.1:2222".to_string(),
            mirrors_root: root.to_path_buf(),
            config_path: root.join("gitquarry.toml"),
            exe_path: root.join("gitquarry"),
            backend: Arc::new(backend) as Arc<dyn Backend>,
            locks: Locks::default(),
            cipher,
            mirror_max_age: Duration::from_secs(60),
            mirror_max_size_bytes: 0,
            mirror_large_repo_threshold_bytes: u64::MAX,
            oidc: None,
            login_attempts: Default::default(),
            git_rate_limiter: Default::default(),
            federation: None,
            allow_push_create: false,
            trust_forwarded_for: false,
            events: Default::default(),
            mirror_sync: None,
            visible_repo_uids_cache: Default::default(),
        }
    }

    let dir_b = tempfile::tempdir().unwrap();
    let ctx_b = Arc::new(test_ctx(dir_b.path()));

    let resolved = ctx_b.create_repo("acme/demo").await.unwrap().unwrap();
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/demo".to_string(),
        visibility: Visibility::Public,
        ..Default::default()
    };
    settings::put_repo_config(
        ctx_b.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx_b.cipher,
    )
    .await
    .unwrap();
    settings::set_visible_index(ctx_b.backend().as_ref(), &ctx_b.cipher, &resolved.uid, true).await;

    // A real commit via a real scratch working tree — simpler and more
    // realistic than hand-building blob/tree/commit objects.
    let work = tempfile::tempdir().unwrap();
    async fn run_git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git").current_dir(dir).args(args).status().await.unwrap();
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }
    run_git(work.path(), &["init", "-q"]).await;
    std::fs::write(work.path().join("README.md"), b"hello from B\n").unwrap();
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
            "init",
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
            &Manifest {
                refs: BTreeMap::from([("refs/heads/main".to_string(), sha.clone())]),
                head: Some("refs/heads/main".to_string()),
            },
            None,
        )
        .await
        .unwrap();

    let bind_addr = "127.0.0.1:18454";
    tokio::spawn(async move {
        // `.fallback()` before `.layer()`: a layer only wraps whatever
        // the router already has at the point it's added, so the
        // middleware has to come last to also cover the fallback route.
        let app = axum::Router::new()
            .fallback(|| async { axum::http::StatusCode::NOT_FOUND })
            .layer(axum::middleware::from_fn_with_state(
                ctx_b,
                gitquarry_core::git::http::middleware,
            ));
        let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let dir_a = tempfile::tempdir().unwrap();
    let ctx_a = test_ctx(dir_a.path());

    let mirror = ctx_a
        .open_remote_mirror_at("instance-b", &format!("http://{bind_addr}"), "acme/demo")
        .await
        .unwrap();
    let head = gitquarry_core::git::browse::resolve_head(&mirror.path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head, "refs/heads/main");
    let log = gitquarry_core::git::browse::log(&mirror.path, "main", 10, None)
        .await
        .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].sha, sha);
}
