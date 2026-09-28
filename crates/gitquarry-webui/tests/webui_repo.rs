//! Every browsing view and raw export of a repo, driven through `router()`
//! against real content pushed into the bucket: the parser unit tests in
//! `git::browse` cover the string-munging in isolation, but only this would
//! catch a mismatch against actual `git` output (wrong flag, wrong field
//! order, a mirror sync that never landed the pushed objects).

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

struct Seeded {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    app: Router,
    first_sha: String,
}

/// `team/proj`, public: an initial commit (README + `src.rs`) with a
/// `feature` branch and an annotated `v1.0` tag on it, then a second commit
/// on `main` that edits `src.rs`.
async fn seeded() -> Seeded {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::write(w.join("README.md"), "# Hello\n\nSome **bold** text.\n").unwrap();
    std::fs::write(w.join("src.rs"), "fn main() {}\n").unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "initial commit"]).await;
    git(w, &["branch", "feature"]).await;
    git(w, &["tag", "-a", "v1.0", "-m", "release notes"]).await;
    let first_sha = git(w, &["rev-parse", "HEAD"]).await.trim().to_string();
    std::fs::write(w.join("src.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    git(w, &["commit", "--quiet", "-am", "add a println"]).await;
    push_all(&ctx, "team/proj", w).await;

    Seeded {
        _dirs: (root, work),
        app: gitquarry_webui::webui::router(ctx),
        first_sha,
    }
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

async fn page(app: &Router, uri: &str) -> String {
    let (status, _, body) = get(app, uri, None).await;
    assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
    body
}

#[tokio::test]
async fn tree_blob_and_blame_render_the_pushed_files() {
    let s = seeded().await;

    let tree = page(&s.app, "/team/proj?view=tree").await;
    assert!(tree.contains("README.md"), "tree: {tree}");
    assert!(tree.contains("src.rs"), "tree: {tree}");

    let blob = page(&s.app, "/team/proj?view=blob&path=src.rs").await;
    assert!(blob.contains("println"), "blob: {blob}");

    let blame = page(&s.app, "/team/proj?view=blame&path=src.rs").await;
    assert!(blame.contains("Test Author"), "blame: {blame}");
    assert!(blame.contains("fn main"), "blame: {blame}");
}

#[tokio::test]
async fn log_and_commit_views_show_real_history() {
    let s = seeded().await;

    let log = page(&s.app, "/team/proj?view=log").await;
    assert!(log.contains("initial commit"), "log: {log}");
    assert!(log.contains("add a println"), "log: {log}");

    let commit = page(&s.app, &format!("/team/proj?view=commit&sha={}", s.first_sha)).await;
    assert!(commit.contains("initial commit"), "commit: {commit}");
    assert!(
        commit.contains(r#"value="split""#),
        "commit missing diff-view toggle: {commit}"
    );

    // The log lists newest first, so its first commit link is `main`'s tip.
    let head_sha = log
        .split("view=commit&amp;sha=")
        .nth(1)
        .and_then(|rest| rest.get(..40))
        .unwrap()
        .to_string();
    let (_, _, split) = get(
        &s.app,
        &format!("/team/proj?view=commit&sha={head_sha}"),
        Some("diff-view=split"),
    )
    .await;
    assert!(split.contains("diff-split"), "split: {split}");
    assert!(split.contains("println"), "split: {split}");
    assert!(split.contains("size-btn active"), "split: {split}");
}

#[tokio::test]
async fn refs_stats_and_about_views() {
    let s = seeded().await;

    let refs = page(&s.app, "/team/proj?view=refs").await;
    assert!(refs.contains("feature"), "refs: {refs}");
    assert!(refs.contains("v1.0"), "refs: {refs}");
    assert!(refs.contains("release notes"), "refs: {refs}");

    let stats = page(&s.app, "/team/proj?view=stats").await;
    assert!(stats.contains("Test Author"), "stats: {stats}");

    let about = page(&s.app, "/team/proj").await;
    assert!(about.contains("<strong>bold</strong>"), "about: {about}");
}

#[tokio::test]
async fn raw_exports_answer_with_their_own_content_types() {
    let s = seeded().await;

    let (status, headers, body) = get(&s.app, "/_raw/plain/team/proj?path=src.rs", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    assert_eq!(body, "fn main() {\n    println!(\"hi\");\n}\n");

    // A missing path errors plain-text, not the normal HTML error page — a
    // raw view can't leak markup into a client expecting raw bytes.
    let (status, headers, _) = get(&s.app, "/_raw/plain/team/proj?path=no-such-file.rs", None).await;
    assert!(status.is_client_error() || status.is_server_error(), "{status}");
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");

    let (status, _, patch) = get(&s.app, &format!("/_raw/patch/team/proj?sha={}", s.first_sha), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(patch.contains("initial commit"), "patch: {patch}");

    let (_, headers, _) = get(&s.app, "/_raw/snapshot/team/proj?fmt=zip", None).await;
    assert_eq!(headers[header::CONTENT_TYPE], "application/zip");

    let (_, headers, _) = get(&s.app, "/_raw/snapshot/team/proj?fmt=tar.gz", None).await;
    assert_eq!(headers[header::CONTENT_TYPE], "application/gzip");

    let (_, headers, _) = get(&s.app, "/_raw/atom/team/proj", None).await;
    assert_eq!(headers[header::CONTENT_TYPE], "application/atom+xml; charset=utf-8");
}
