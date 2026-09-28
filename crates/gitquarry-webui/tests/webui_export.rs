//! The headers `/_raw/*` answers with for real pushed files: which types
//! render inline, which are forced to a download, and that a filename can
//! never break out of its `Content-Disposition` quoted-string.

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
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[21u8; s3db_crypto::SALT_LEN]).unwrap());
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

struct Seeded {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    app: Router,
    sha: String,
}

/// Public `team/proj` with one commit holding one file of every kind the
/// raw view treats differently, pushed the way `git push` leaves a repo.
async fn seeded() -> Seeded {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::create_dir_all(w.join("assets")).unwrap();
    std::fs::create_dir_all(w.join("docs")).unwrap();
    for (name, content) in [
        ("README.md", &b"# Hello\n"[..]),
        ("page.html", b"<script>alert(1)</script>\n"),
        ("logo.png", b"\x89PNG\r\n\x1a\n"),
        ("binary.bin", b"\x00\x01\x02"),
        (
            "assets/logo.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
        ),
        ("docs/spec.pdf", b"%PDF-1.4\n"),
        ("weird\"name.svg", b"<svg/>"),
        ("back\\slash.svg", b"<svg/>"),
    ] {
        std::fs::write(w.join(name), content).unwrap();
    }
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "every kind of file"]).await;
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
                refs: [("refs/heads/main".to_string(), sha.clone())].into(),
                head: Some("refs/heads/main".to_string()),
            },
            etag.as_deref(),
        )
        .await
        .unwrap();

    Seeded {
        _dirs: (root, work),
        app: gitquarry_webui::webui::router(ctx),
        sha,
    }
}

async fn get(app: &Router, uri: &str) -> (StatusCode, HeaderMap, String) {
    let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// `(Content-Type, Content-Disposition)` of the raw view of `path`, which
/// must already be percent-encoded for the query string.
async fn raw_headers(app: &Router, path: &str) -> (String, Option<String>) {
    let (status, headers, body) = get(app, &format!("/_raw/plain/team/proj?path={path}")).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    let content_type = headers[header::CONTENT_TYPE].to_str().unwrap().to_string();
    let disposition = headers
        .get(header::CONTENT_DISPOSITION)
        .map(|v| v.to_str().unwrap().to_string());
    (content_type, disposition)
}

#[tokio::test]
async fn common_extensions_get_their_content_type_and_render_inline() {
    let s = seeded().await;

    assert_eq!(raw_headers(&s.app, "logo.png").await, ("image/png".to_string(), None));
    assert_eq!(
        raw_headers(&s.app, "README.md").await,
        ("text/plain; charset=utf-8".to_string(), None)
    );
    assert_eq!(
        raw_headers(&s.app, "binary.bin").await,
        ("application/octet-stream".to_string(), None)
    );
    // Served as text, never as `text/html`: the raw view is same-origin, so
    // a pushed page would otherwise run its script against this app.
    assert_eq!(
        raw_headers(&s.app, "page.html").await,
        ("text/plain; charset=utf-8".to_string(), None)
    );
}

#[tokio::test]
async fn blob_view_embeds_images_and_prints_code() {
    let s = seeded().await;

    let (status, _, svg) = get(&s.app, "/team/proj?view=blob&path=assets/logo.svg").await;
    assert_eq!(status, StatusCode::OK, "{svg}");
    assert!(svg.contains(r#"<img src="/_raw/plain/"#), "{svg}");
    assert!(!svg.contains(r#"<pre class="blob">"#), "{svg}");

    let (status, _, readme) = get(&s.app, "/team/proj?view=blob&path=README.md").await;
    assert_eq!(status, StatusCode::OK, "{readme}");
    assert!(!readme.contains(r#"<img src="/_raw/plain/"#), "{readme}");
    assert!(readme.contains(r#"<pre class="blob">"#), "{readme}");
}

#[tokio::test]
async fn svg_is_forced_to_a_download_to_block_inline_script_execution() {
    let s = seeded().await;

    assert_eq!(
        raw_headers(&s.app, "assets/logo.svg").await,
        (
            "image/svg+xml".to_string(),
            Some("attachment; filename=\"logo.svg\"".to_string())
        )
    );
}

/// PDF keeps `application/pdf` (an `<embed>` needs it) but must not render
/// inline from the raw URL: a PDF can carry JavaScript, and the viewer that
/// runs it sits largely outside what this app's CSP can reach.
#[tokio::test]
async fn raw_pdf_is_handed_over_as_a_download() {
    let s = seeded().await;

    assert_eq!(
        raw_headers(&s.app, "docs/spec.pdf").await,
        (
            "application/pdf".to_string(),
            Some("attachment; filename=\"spec.pdf\"".to_string())
        )
    );
}

/// A quote in a tree path used to close the quoted-string early and let the
/// rest of the name read as header parameters.
#[tokio::test]
async fn disposition_filename_stays_inside_its_quoted_string() {
    let s = seeded().await;

    let (_, disposition) = raw_headers(&s.app, "weird%22name.svg").await;
    assert_eq!(disposition.as_deref(), Some("attachment; filename=\"weird'name.svg\""));
    let (_, disposition) = raw_headers(&s.app, "back%5Cslash.svg").await;
    assert_eq!(disposition.as_deref(), Some("attachment; filename=\"back'slash.svg\""));
}

#[tokio::test]
async fn a_control_character_in_the_path_never_reaches_a_header() {
    let s = seeded().await;

    let (status, headers, _) = get(&s.app, "/_raw/plain/team/proj?path=bad%0D%0Aname.svg").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(headers.get(header::CONTENT_DISPOSITION).is_none(), "{headers:?}");
}

#[tokio::test]
async fn patch_and_snapshot_downloads_name_their_file() {
    let s = seeded().await;

    let (status, headers, _) = get(&s.app, &format!("/_raw/patch/team/proj?sha={}", s.sha)).await;
    assert_eq!(status, StatusCode::OK);
    let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.starts_with("attachment; filename=\""), "{disposition}");
    assert!(disposition.contains(&s.sha[..7]), "{disposition}");
    assert!(disposition.ends_with(".patch\""), "{disposition}");

    let (status, headers, _) = get(&s.app, "/_raw/snapshot/team/proj?fmt=zip").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"proj-main.zip\""
    );
}
