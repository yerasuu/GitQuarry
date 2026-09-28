//! The in-repo links pages hand out (`?view=` query strings and GitHub-style
//! `/blob/` paths): each checked for its exact shape and then followed, so
//! the link builder and the route/view dispatch are checked against each
//! other.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::{Manifest, ReleaseMeta};
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

/// `team/proj` with `src/lib.rs` on `main`, a `feature` branch, tag `v1.0`,
/// and a release on it.
async fn seeded() -> (tempfile::TempDir, tempfile::TempDir, Router, String) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let resolved = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::create_dir(w.join("src")).unwrap();
    std::fs::write(
        w.join("src/lib.rs"),
        "pub fn answer() -> u32 { 42 }
",
    )
    .unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "initial commit"]).await;
    git(w, &["branch", "feature"]).await;
    git(w, &["tag", "v1.0"]).await;
    let sha = git(w, &["rev-parse", "HEAD"]).await.trim().to_string();
    push_all(&ctx, "team/proj", w).await;

    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "First release".to_string(),
        notes: String::new(),
        assets: Vec::new(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: false,
    };
    resolved.remote.put_release(&release, None).await.unwrap();

    (root, work, gitquarry_webui::webui::router(ctx), sha)
}

/// The first `href` in `html` starting with `prefix`, unescaped the way a
/// browser reads the attribute before following it.
fn href(html: &str, prefix: &str) -> String {
    let needle = format!(r#"href="{prefix}"#);
    let start = html
        .find(&needle)
        .unwrap_or_else(|| panic!("no href starting {prefix}: {html}"))
        + 6;
    let len = html[start..].find('"').unwrap();
    html[start..start + len].replace("&amp;", "&")
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
async fn the_tree_links_a_file_by_its_github_style_blob_path() {
    let (_root, _work, app, _) = seeded().await;
    let (_, _, html) = get(&app, "/team/proj?view=tree&path=src", None).await;
    let link = href(&html, "/team/proj/blob/");
    assert_eq!(link, "/team/proj/blob/main/src/lib.rs");

    let (status, _, body) = get(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("answer"), "{body}");
}

#[tokio::test]
async fn the_log_links_each_commit_by_sha() {
    let (_root, _work, app, sha) = seeded().await;
    let (_, _, html) = get(&app, "/team/proj?view=log", None).await;
    let link = href(&html, "/team/proj?view=commit");
    assert_eq!(link, format!("/team/proj?view=commit&sha={sha}"));

    let (status, _, body) = get(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("initial commit"), "{body}");
}

#[tokio::test]
async fn the_refs_page_links_a_branch_tree_without_empty_fields() {
    let (_root, _work, app, _) = seeded().await;
    let (_, _, html) = get(&app, "/team/proj?view=refs", None).await;
    let link = href(&html, "/team/proj?view=tree&amp;ref=feature");
    assert_eq!(link, "/team/proj?view=tree&ref=feature");

    let (status, _, body) = get(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("src"), "{body}");
}

#[tokio::test]
async fn the_releases_list_links_each_release_by_tag() {
    let (_root, _work, app, _) = seeded().await;
    let (_, _, html) = get(&app, "/team/proj?view=releases", None).await;
    let link = href(&html, "/team/proj?view=release&amp;tag=");
    assert_eq!(link, "/team/proj?view=release&tag=v1.0");

    let (status, _, body) = get(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("First release"), "{body}");
}
