//! A README as the about tab renders it. A README is untrusted content —
//! anyone with write access controls what every other viewer's browser
//! gets — so the sanitizing properties are pinned on the served page.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
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
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[23u8; s3db_crypto::SALT_LEN]).unwrap());
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

/// Pushes a public `team/proj` whose only file is a `README.md` holding
/// `markdown`, and returns the rendered README out of its about tab — just
/// that element, since the page around it has its own scripts and links.
async fn readme(markdown: &str) -> String {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::write(w.join("README.md"), markdown).unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "readme"]).await;
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

    let mut request = Request::builder().uri("/team/proj").body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = gitquarry_webui::webui::router(ctx).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let page = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(status, StatusCode::OK, "{page}");

    let start = page
        .find(r#"<div id="readme" class="readme">"#)
        .unwrap_or_else(|| panic!("no rendered readme: {page}"));
    let len = page[start..].find("</div>").unwrap();
    page[start..start + len].to_string()
}

#[tokio::test]
async fn renders_basic_markdown() {
    let html = readme("# Title\n\nSome **bold** text.").await;
    assert!(html.contains("<h1>Title</h1>"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
}

#[tokio::test]
async fn strips_raw_html() {
    let html = readme("before <script>alert(1)</script> after\n\n<img src=x onerror=alert(1)>\n").await;
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(html.contains("before"), "{html}");
}

#[tokio::test]
async fn neutralizes_javascript_link() {
    let html = readme("[click me](javascript:alert(1))").await;
    assert!(!html.contains("javascript:"), "{html}");
    assert!(html.contains("href=\"#\""), "{html}");
}

#[tokio::test]
async fn keeps_safe_links() {
    let html = readme("[repo](https://example.com/x) and [rel](./docs)").await;
    assert!(html.contains("href=\"https://example.com/x\""), "{html}");
    assert!(html.contains("href=\"./docs\""), "{html}");
}

#[tokio::test]
async fn highlights_a_fenced_code_block_with_a_language_tag() {
    let html = readme("```rust\nfn main() {}\n```").await;
    assert!(html.contains("<pre class=\"blob\">"), "{html}");
    // The fence's language token becomes the `language-*` class
    // `highlight.js` (client-side, vendored) looks for -- no server-side
    // tokenizing happens, see `markdown.rs`'s own doc comment.
    assert!(html.contains("class=\"language-rust\""), "{html}");
    assert!(html.contains("fn main() {}"), "{html}");
}

#[tokio::test]
async fn does_not_highlight_a_fence_with_no_language_tag() {
    let html = readme("```\nfn main() {}\n```").await;
    assert!(!html.contains("<pre class=\"blob\">"), "{html}");
    assert!(html.contains("fn main() {}"), "{html}");
}

#[tokio::test]
async fn highlighted_code_block_still_escapes_html_metacharacters() {
    let html = readme("```rust\nlet x = a < b;\n```").await;
    assert!(!html.contains("a < b"), "{html}");
    assert!(html.contains("a &lt; b"), "{html}");
}

#[tokio::test]
async fn renders_github_style_alert_blockquotes() {
    let html = readme("> [!NOTE]\n> Something worth knowing.\n\n> [!WARNING]\n> Be careful.").await;
    assert!(html.contains("class=\"markdown-alert-note\""), "{html}");
    assert!(html.contains("class=\"markdown-alert-warning\""), "{html}");
    // The marker line itself is parsed away, not left as literal text.
    assert!(!html.contains("[!NOTE]"), "{html}");
    assert!(html.contains("Something worth knowing."), "{html}");
}

#[tokio::test]
async fn plain_blockquote_is_unaffected() {
    let html = readme("> just a quote, not an alert").await;
    assert!(html.contains("<blockquote>"), "{html}");
    assert!(!html.contains("markdown-alert"), "{html}");
}
