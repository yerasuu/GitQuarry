use gitquarry_core::mirror::ensure_bare_repo;

// The hook script embeds the binary's absolute path; a space or a `'` in it
// must neither split the command nor inject into it, so run the installed
// hook for real against an executable whose path contains both.
#[tokio::test]
async fn pre_receive_hook_runs_an_exe_whose_path_has_spaces_and_quotes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let exe_dir = dir.path().join("a b'c");
    std::fs::create_dir(&exe_dir).unwrap();
    let exe = exe_dir.join("fake gitquarry");
    let out = dir.path().join("called-with");
    std::fs::write(&exe, format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", out.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bare = dir.path().join("repo.git");

    ensure_bare_repo(&bare, &exe).await.unwrap();

    let hook = bare.join("hooks/pre-receive");
    assert_eq!(std::fs::metadata(&hook).unwrap().permissions().mode() & 0o777, 0o700);
    let status = std::process::Command::new(&hook).status().unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "pre-receive-hook");
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
    use gitquarry_core::storage::entities::{RepoConfig, Visibility};
    use gitquarry_core::storage::settings;
    use std::sync::Arc;
    use std::time::Duration;

    let dir_b = tempfile::tempdir().unwrap();
    let ctx_b = Arc::new(gitquarry_test_support::ctx(dir_b.path()));

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
    let work = gitquarry_test_support::Work::new().await;
    let sha = work.commit(&[("README.md", "hello from B\n")], "init").await;
    work.push(&resolved, "refs/heads/main").await;

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
    let ctx_a = gitquarry_test_support::ctx(dir_a.path());

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
