use gitquarry_core::domain::remote_fork::*;
use gitquarry_core::storage;
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;

/// Same end-to-end shape as the old `fork_from_peer` test this replaces:
/// spins up a real "instance B" over a real socket, seeds one public
/// repo with real git history, then confirms `seed_from_remote` (called
/// with just B's git base URL, no `Peer` type at all) produces an
/// independently-readable copy on "instance A"'s own bucket.
#[tokio::test]
async fn seed_from_remote_copies_a_real_repo_from_a_real_remote() {
    use gitquarry_core::storage::entities::Visibility;

    let dir_b = tempfile::tempdir().unwrap();
    let ctx_b = Arc::new(gitquarry_test_support::ctx(dir_b.path()));

    let source = ctx_b.create_repo("acme/demo").await.unwrap().unwrap();
    let config = gitquarry_core::storage::entities::RepoConfig {
        uid: source.uid.clone(),
        full_name: "acme/demo".to_string(),
        visibility: Visibility::Public,
        ..Default::default()
    };
    storage::settings::put_repo_config(
        ctx_b.backend().as_ref(),
        source.uid.as_str(),
        &config,
        None,
        &ctx_b.cipher,
    )
    .await
    .unwrap();
    storage::settings::set_visible_index(ctx_b.backend().as_ref(), &ctx_b.cipher, &source.uid, true).await;

    let work = gitquarry_test_support::Work::new().await;
    let sha = work.commit(&[("README.md", "hello from B\n")], "init").await;
    work.push(&source, "refs/heads/main").await;

    let bind_addr = "127.0.0.1:18458";
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
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let dir_a = tempfile::tempdir().unwrap();
    let ctx_a = gitquarry_test_support::ctx(dir_a.path());

    let dest_owner = gitquarry_core::storage::entities::OwnerRef::User(Uid::from("alice-uid".to_string()));
    let dest = seed_from_remote(
        &ctx_a,
        SeedFromRemote {
            source_git_base_url: &format!("http://{bind_addr}"),
            source_repo_name: "acme/demo",
            dest_full_name: "alice/demo",
            dest_owner,
            description: Some("forked description".to_string()),
            peer_label: "instance-b",
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap()
    .expect("seeding from a real, reachable remote must succeed");

    let (new_config, etag) =
        storage::settings::get_repo_config(ctx_a.backend().as_ref(), dest.uid.as_str(), &ctx_a.cipher)
            .await
            .unwrap();
    assert!(etag.is_some());
    assert_eq!(new_config.full_name, "alice/demo");
    assert_eq!(new_config.description.as_deref(), Some("forked description"));
    assert_eq!(new_config.visibility, Visibility::Public);
    assert_eq!(
        new_config.forked_from_peer,
        Some(gitquarry_core::storage::entities::PeerOrigin {
            peer: "instance-b".to_string(),
            repo: "acme/demo".to_string()
        })
    );
    assert_eq!(new_config.forked_from, None);

    let (manifest, _etag) = dest.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/heads/main"), Some(&sha));

    let mirror = ctx_a.open_mirror(&dest).await.unwrap();
    let log = gitquarry_core::git::browse::log(&mirror.path, "main", 10, None)
        .await
        .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].sha, sha);
}
