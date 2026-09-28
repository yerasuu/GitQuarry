use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use gitquarry_webui::webui::esc;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

#[test]
fn escapes_html_metacharacters() {
    assert_eq!(esc("<script>&\"'"), "&lt;script&gt;&amp;&quot;&#39;");
}

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[22u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
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

async fn git(dir: &Path, args: &[&str]) -> String {
    // Signing is forced off so a developer's global `commit.gpgsign` can't
    // turn every commit here into a pinentry prompt.
    let out = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A tree entry's name reaches its href percent-encoded — everything
/// outside RFC 3986's unreserved set, but never the `/` between repo path
/// segments — and the encoded link routes back to the same file.
#[tokio::test]
async fn tree_links_percent_encode_names_but_keep_path_separators() {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/project", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::write(w.join("a b?c.txt"), "spaced out\n").unwrap();
    std::fs::create_dir_all(w.join("my dir")).unwrap();
    std::fs::write(w.join("my dir/inner.txt"), "inner\n").unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "names that need encoding"]).await;
    let sha = git(w, &["rev-parse", "HEAD"]).await;
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(&w.join(".git"), &sha, &[], packs.path())
        .await
        .unwrap()
        .unwrap();
    repo.remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let (_, etag) = repo.remote.get_manifest().await.unwrap();
    repo.remote
        .put_manifest(
            &Manifest {
                refs: [("refs/heads/main".to_string(), sha)].into(),
                head: Some("refs/heads/main".to_string()),
            },
            etag.as_deref(),
        )
        .await
        .unwrap();
    let app = gitquarry_webui::webui::router(ctx);

    let (status, tree) = get(&app, "/team/project?view=tree").await;
    assert_eq!(status, StatusCode::OK, "{tree}");
    assert!(
        tree.contains(r#"href="/team/project/blob/main/a%20b%3Fc.txt""#),
        "{tree}"
    );
    assert!(
        tree.contains(r#"href="/team/project?view=tree&amp;ref=main&amp;path=my%20dir""#),
        "{tree}"
    );

    let (status, blob) = get(&app, "/team/project/blob/main/a%20b%3Fc.txt").await;
    assert_eq!(status, StatusCode::OK, "{blob}");
    assert!(blob.contains("spaced out"), "{blob}");
}
