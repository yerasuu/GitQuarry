//! The unified/split diff preference: which layout a page draws from the
//! `diff-view` cookie, the toggle buttons above the diff, and the
//! `POST /_prefs/diff_view` target that sets the cookie.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const SALT_BYTE: u8 = 47;

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[SALT_BYTE; s3db_crypto::SALT_LEN]).unwrap(),
    );
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Pushes `work`'s whole history and every ref it has into a fresh public
/// `team/proj` as a real pack, so the server's own mirror serves it.
async fn publish(root: &Path, work: &Path) -> (Arc<AppContext>, Router) {
    let ctx = Arc::new(test_ctx(root));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(
        &work.join(".git"),
        &git(work, &["rev-parse", "HEAD"]),
        &[],
        packs.path(),
    )
    .await
    .unwrap()
    .unwrap();
    repo.remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let refs = git(work, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let manifest = Manifest {
        refs: refs
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(name, sha)| (name.to_string(), sha.to_string()))
            .collect(),
        head: Some("refs/heads/main".to_string()),
    };
    let (_, etag) = repo.remote.get_manifest().await.unwrap();
    assert_eq!(
        repo.remote.put_manifest(&manifest, etag.as_deref()).await.unwrap(),
        Cas::Ok
    );
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    (ctx, app)
}

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> String {
    let mut builder = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    // `run_server` serves with connect info; a bare `oneshot` doesn't, and
    // the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(status, StatusCode::OK, "{uri}: {html}");
    html
}

/// One commit, so the commit page has a diff to draw.
async fn fixture() -> (tempfile::TempDir, Router, String) {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("f.txt"), "hello\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "initial"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    let (_ctx, app) = publish(dir.path(), &work).await;
    (dir, app, sha)
}

async fn toggle(view: &str, return_to: &str) -> (StatusCode, HeaderMap) {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(dir.path())));
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("view", view), ("return_to", return_to)])
        .finish();
    let mut request = Request::builder()
        .method("POST")
        .uri("/_prefs/diff_view")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.oneshot(request).await.unwrap();
    (response.status(), response.headers().clone())
}

fn is_split(html: &str) -> bool {
    let split = html.contains(r#"<table class="diff-split">"#);
    let unified = html.contains(r#"<span class="diff-hunk">"#);
    assert_ne!(split, unified, "exactly one layout should render: {html}");
    split
}

#[tokio::test]
async fn the_diff_is_unified_unless_the_cookie_picks_split() {
    let (_dir, app, sha) = fixture().await;
    let uri = format!("/team/proj?view=commit&sha={sha}");
    assert!(!is_split(&get(&app, &uri, None).await));
    assert!(is_split(&get(&app, &uri, Some("diff-view=split")).await));
    assert!(!is_split(&get(&app, &uri, Some("diff-view=nonsense")).await));
}

#[tokio::test]
async fn diff_view_buttons_mark_the_current_one_active() {
    let (_dir, app, sha) = fixture().await;
    let uri = format!("/team/proj?view=commit&sha={sha}");
    let html = get(&app, &uri, Some("diff-view=split")).await;
    let forms: Vec<&str> = html
        .split(r#"<form method="post" action="/_prefs/diff_view""#)
        .skip(1)
        .map(|rest| rest.split("</form>").next().unwrap())
        .collect();
    assert_eq!(forms.len(), 2, "{html}");
    let (unified, split) = (forms[0], forms[1]);
    assert!(unified.contains(r#"name="view" value="unified""#), "{unified}");
    assert!(split.contains(r#"name="view" value="split""#), "{split}");
    assert!(!unified.contains("size-btn active"), "{unified}");
    assert!(split.contains("size-btn active"), "{split}");
    // Each button brings the reader back to the page they flipped it on.
    let return_to = format!(r#"name="return_to" value="/team/proj?view=commit&amp;sha={sha}""#);
    assert!(forms.iter().all(|form| form.contains(&return_to)), "{html}");
}

#[tokio::test]
async fn toggle_diff_view_sets_the_explicit_pick_and_redirects() {
    let (status, headers) = toggle("split", "/acme/proj").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/acme/proj");
    let set_cookie = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("diff-view=split;"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");

    // Anything but the two known values is stored as the default.
    let (_, headers) = toggle("sideways", "/").await;
    assert!(
        headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("diff-view=unified;")
    );
}

#[tokio::test]
async fn toggle_diff_view_rejects_an_off_site_return_to() {
    for return_to in ["//evil.example", "/\\evil.example", "https://evil.example", ""] {
        let (status, headers) = toggle("unified", return_to).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(headers[header::LOCATION], "/", "{return_to:?}");
    }
}
