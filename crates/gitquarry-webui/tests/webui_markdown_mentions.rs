//! What an MR body — a description, a comment, a review, an inline note —
//! survives on its way to the MR page.
//!
//! Two things had to hold at once, and they merge against each other: the
//! body is markdown written by a person, and `#123` in it should reach a
//! merge request. Substituting the link into the *source text* satisfies the
//! second and breaks the first, because a `#123` inside a code fence is not
//! a mention. So the substitution happens on the parsed event stream, and
//! these pin the cases that distinguishes.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::domain::merge_request::{self, NewMergeRequest};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use gitquarry_i18n::Locale;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

/// What `#1` becomes once it resolves to MR #1 of `team/proj`.
const MR1_LINK: &str = r#"<a href="/team/proj?view=merge&amp;number=1">#1</a>"#;

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[24u8; s3db_crypto::SALT_LEN]).unwrap());
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

struct Fixture {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    ctx: Arc<AppContext>,
    repo: ResolvedRepo,
}

/// Public `team/proj` whose `feature` branch is one commit ahead of `main`,
/// with MR #1 open between them carrying `description`.
async fn fixture(description: &str) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::write(w.join("f.txt"), "hello\n").unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "initial"]).await;
    let main_sha = git(w, &["rev-parse", "HEAD"]).await;
    git(w, &["checkout", "--quiet", "-b", "feature"]).await;
    std::fs::write(w.join("f.txt"), "hello\nworld\n").unwrap();
    git(w, &["commit", "--quiet", "-am", "add world"]).await;
    let feature_sha = git(w, &["rev-parse", "HEAD"]).await;

    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(&w.join(".git"), &feature_sha, &[], packs.path())
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
                refs: [
                    ("refs/heads/main".to_string(), main_sha),
                    ("refs/heads/feature".to_string(), feature_sha),
                ]
                .into(),
                head: Some("refs/heads/main".to_string()),
            },
            etag.as_deref(),
        )
        .await
        .unwrap();

    let new = NewMergeRequest {
        source_repo: &repo.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description,
        author: "alice",
    };
    merge_request::create(&ctx, &repo, new, Locale::En).await.unwrap();

    Fixture {
        _dirs: (root, work),
        ctx,
        repo,
    }
}

/// Every rendered body on MR #1's page, in page order (description first),
/// cut out of the page so the page's own links don't count.
async fn bodies(f: &Fixture) -> Vec<String> {
    let mut request = Request::builder()
        .uri("/team/proj?view=merge&number=1")
        .body(Body::empty())
        .unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = gitquarry_webui::webui::router(Arc::clone(&f.ctx))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let page = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(status, StatusCode::OK, "{page}");

    page.split(r#"<div class="readme plain">"#)
        .skip(1)
        .map(|rest| rest[..rest.find("</div>").unwrap()].to_string())
        .collect()
}

async fn description(body: &str) -> String {
    let f = fixture(body).await;
    bodies(&f).await.swap_remove(0)
}

#[tokio::test]
async fn a_mention_in_prose_becomes_a_link() {
    let html = description("see #1 for the fix").await;

    assert!(html.contains(MR1_LINK), "{html}");
}

/// The reason this moved out of the source text: a fence is code, and the
/// author wrote those characters on purpose.
#[tokio::test]
async fn a_mention_inside_a_code_fence_stays_literal() {
    let html = description("```\ngit show #1\n```").await;

    assert!(!html.contains("<a href"), "{html}");
    assert!(html.contains("git show #1"), "{html}");
}

/// Same for a backtick span, which the parser hands over as its own event
/// rather than as text.
#[tokio::test]
async fn a_mention_inside_inline_code_stays_literal() {
    let html = description("run `deploy --mr #1` first").await;

    assert!(!html.contains("<a href"), "{html}");
    assert!(html.contains("<code>deploy --mr #1</code>"), "{html}");
}

/// An `<a>` inside an `<a>` isn't valid HTML, and the author already said
/// where that text should point.
#[tokio::test]
async fn a_mention_inside_an_existing_link_is_left_alone() {
    let html = description("[see #1](https://example.com/x)").await;

    assert_eq!(html.matches("<a href").count(), 1, "{html}");
    assert!(html.contains("https://example.com/x"), "{html}");
}

/// A number that names no merge request is not a dead link.
#[tokio::test]
async fn an_unresolved_number_stays_plain_text() {
    let html = description("see #999 and #1").await;

    assert_eq!(html.matches("<a href").count(), 1, "{html}");
    assert!(html.contains(MR1_LINK), "{html}");
    assert!(html.contains("#999"), "{html}");
}

/// The regression that started this: a body written as a list used to reach
/// the page as one flat paragraph with its bullets showing as literal `-`.
/// A thread comment goes through the same rendering as the description.
#[tokio::test]
async fn a_comment_written_as_markdown_renders_as_markdown() {
    let f = fixture("d").await;
    merge_request::add_comment(
        &f.ctx,
        &f.repo,
        1,
        "bob",
        "Highlights:\n\n- first\n- second\n\n`inline code` and **bold**, see #1.",
        Locale::En,
    )
    .await
    .unwrap();

    let html = bodies(&f).await.into_iter().find(|b| b.contains("Highlights")).unwrap();
    assert!(html.contains("<ul>"), "{html}");
    assert_eq!(html.matches("<li>").count(), 2, "{html}");
    assert!(html.contains("<code>inline code</code>"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
    assert!(html.contains(MR1_LINK), "{html}");
}

/// Moving the escaping into the renderer must not have moved it out of the
/// way: a body is still untrusted text from anyone who can comment.
#[tokio::test]
async fn raw_html_in_a_body_is_still_stripped() {
    let html = description("hello <script>alert(1)</script> world, see #1").await;

    assert!(!html.contains("<script"), "{html}");
    assert!(html.contains("hello"), "{html}");
}
