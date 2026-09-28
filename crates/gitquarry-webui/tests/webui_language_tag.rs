//! The `language-*` class a blob's `<code>` carries, which is what
//! `highlight.js` tokenizes it by.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::remote::Manifest;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

/// Same shape as `webui_routes.rs`'s own `test_ctx`, duplicated because
/// every file under `tests/` is its own crate.
fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
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

async fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = tokio::process::Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `acme/proj` with one commit on `main` holding `files`, pushed the way
/// receive-pack stores it: a pack plus the manifest pointing at it.
async fn app_with_files(root: &std::path::Path, files: &[&str]) -> axum::Router {
    let work = tempfile::tempdir().unwrap();
    git(work.path(), &["init", "-q", "-b", "main"]).await;
    for file in files {
        let path = work.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x\n").unwrap();
    }
    git(work.path(), &["add", "-A"]).await;
    git(work.path(), &["commit", "-q", "-m", "files"]).await;
    let tip = git(work.path(), &["rev-parse", "HEAD"]).await;

    let ctx = test_ctx(root);
    let repo = ctx.create_repo("acme/proj").await.unwrap().unwrap();
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(&work.path().join(".git"), &tip, &[], packs.path())
        .await
        .unwrap()
        .unwrap();
    repo.remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    repo.remote
        .put_manifest(
            &Manifest {
                refs: BTreeMap::from([("refs/heads/main".to_string(), tip)]),
                head: Some("refs/heads/main".to_string()),
            },
            None,
        )
        .await
        .unwrap();
    gitquarry_webui::webui::router(Arc::new(ctx))
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't add it, and its absence 500s before routing.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    app.clone().oneshot(request).await.unwrap()
}

async fn blob_code_class(app: &axum::Router, path: &str) -> String {
    let request = Request::get(format!("/acme/proj?view=blob&path={path}"))
        .body(Body::empty())
        .unwrap();
    let response = send(app, request).await;
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    let class = html.split(r#"<code id="blob-code" class=""#).nth(1).unwrap();
    class[..class.find('"').unwrap()].to_string()
}

#[tokio::test]
async fn blob_code_is_tagged_with_its_lowercased_extension() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_files(dir.path(), &["src/main.rs", "README.MD", "Dockerfile"]).await;

    assert_eq!(blob_code_class(&app, "src/main.rs").await, "language-rs");
    assert_eq!(blob_code_class(&app, "README.MD").await, "language-md");
    // No extension: `highlight.js`'s own built-in name for "don't tokenize".
    assert_eq!(blob_code_class(&app, "Dockerfile").await, "language-plaintext");
}
