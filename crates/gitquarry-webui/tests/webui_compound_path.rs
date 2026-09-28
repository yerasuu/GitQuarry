//! GitHub-style compound URLs (`/{repo}/blob/{branch}/{path}` and
//! `/_raw/plain/{repo}/refs/heads/{branch}/{path}`), where a branch name
//! may itself contain `/`: the split has to follow the refs that really
//! exist, never a fixed segment count.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

fn test_ctx(root: &Path) -> AppContext {
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
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2024-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2024-01-01T00:00:00Z")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// What a `git push` of every ref in `work` leaves in the bucket: its
/// objects as one uploaded pack, its refs in the manifest.
async fn push_all(ctx: &AppContext, name: &str, work: &Path) {
    let resolved = ctx.resolve_repo(name).await.unwrap().unwrap();
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_all_objects(&work.join(".git"), packs.path())
        .await
        .unwrap()
        .unwrap();
    resolved
        .remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let refs = git(work, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .await
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(name, sha)| (name.to_string(), sha.to_string()))
        .collect();
    let (_, etag) = resolved.remote.get_manifest().await.unwrap();
    resolved
        .remote
        .put_manifest(
            &Manifest {
                refs,
                head: Some("refs/heads/main".to_string()),
            },
            etag.as_deref(),
        )
        .await
        .unwrap();
}

/// `team/proj` with `v1/f.txt` ("nested") and `f.txt` ("top") on `main`,
/// plus `branch` created off it.
async fn seeded(branch: &str) -> (tempfile::TempDir, tempfile::TempDir, Router) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::create_dir(w.join("v1")).unwrap();
    std::fs::write(w.join("v1/f.txt"), "nested").unwrap();
    std::fs::write(w.join("f.txt"), "top").unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "init"]).await;
    git(w, &["branch", branch]).await;
    push_all(&ctx, "team/proj", w).await;
    (root, work, gitquarry_webui::webui::router(ctx))
}

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn splits_branch_and_path_by_checking_real_refs() {
    // Branch "release" exists (not "release/v1" — git's ref namespace can't
    // hold both at once, same as it can't hold both a file and a directory
    // at one path): "release/v1/f.txt" must split as branch="release",
    // path="v1/f.txt".
    let (_root, _work, app) = seeded("release").await;

    let (status, _, raw) = get(&app, "/_raw/plain/team/proj/refs/heads/release/v1/f.txt", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(raw, "nested");

    let (status, _, html) = get(&app, "/team/proj/blob/release/v1/f.txt", None).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("nested"), "{html}");
}

#[tokio::test]
async fn falls_back_to_the_longer_ref_when_the_short_prefix_is_not_one() {
    // Only "release/v1" exists as a branch (no bare "release"), so the same
    // segments must split the other way: branch="release/v1", path="f.txt".
    let (_root, _work, app) = seeded("release/v1").await;

    let (status, _, raw) = get(&app, "/_raw/plain/team/proj/refs/heads/release/v1/f.txt", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(raw, "top");

    let (status, _, html) = get(&app, "/team/proj/blob/release/v1/f.txt", None).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("top"), "{html}");
    assert!(!html.contains("nested"), "{html}");
}

#[tokio::test]
async fn a_private_repo_is_not_reachable_through_the_compound_form() {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    gitquarry_core::domain::repo::create(&ctx, "team/secret", None, None, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap();
    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::write(w.join("f.txt"), "classified").unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "init"]).await;
    push_all(&ctx, "team/secret", w).await;
    let app = gitquarry_webui::webui::router(ctx);

    for uri in [
        "/team/secret/blob/main/f.txt",
        "/_raw/plain/team/secret/refs/heads/main/f.txt",
    ] {
        let (status, _, body) = get(&app, uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
        assert!(!body.contains("classified"), "GET {uri}: {body}");
    }
}
