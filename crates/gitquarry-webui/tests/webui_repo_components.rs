//! The diff and commit-table renderings shared by the repo views, read off
//! the pages that show them: the commit page's unified and side-by-side
//! diffs, and the tree page's recent-commits table.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const SALT_BYTE: u8 = 43;

const LEAD: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\n";

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

/// `main` is `start` -> `before` (tagged `v1.0`) -> `rework`, and `rework`
/// touches one file per diff shape worth telling apart: `notes` (no
/// extension) loses two lines for one, `src/main.rs` changes its last line
/// so its hunk starts mid-file, `gone.go` is deleted (`+++ /dev/null`),
/// `added.py` is new (`--- /dev/null`), and `a b.rs` has a space in its
/// name, which makes git end its `---`/`+++` headers with a tab.
struct Fixture {
    _dir: tempfile::TempDir,
    app: Router,
    rework: String,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join("src")).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("README"), "hi\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "start"]);

    std::fs::write(work.join("notes"), "old one\nold two\ncontext\n").unwrap();
    std::fs::write(work.join("src/main.rs"), format!("{LEAD}gone\n")).unwrap();
    std::fs::write(work.join("gone.go"), "package gone\n").unwrap();
    std::fs::write(work.join("a b.rs"), "a\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "before"]);
    git(&work, &["tag", "v1.0"]);

    std::fs::write(work.join("notes"), "new one\ncontext\n").unwrap();
    std::fs::write(work.join("src/main.rs"), format!("{LEAD}added\n")).unwrap();
    std::fs::remove_file(work.join("gone.go")).unwrap();
    std::fs::write(work.join("added.py"), "print()\n").unwrap();
    std::fs::write(work.join("a b.rs"), "a\nb\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "rework"]);
    let rework = git(&work, &["rev-parse", "HEAD"]);

    let (_ctx, app) = publish(dir.path(), &work).await;
    Fixture { _dir: dir, app, rework }
}

async fn commit_page(f: &Fixture, cookie: Option<&str>) -> String {
    get(&f.app, &format!("/team/proj?view=commit&sha={}", f.rework), cookie).await
}

#[tokio::test]
async fn unified_diff_headers_win_over_add_del() {
    let f = fixture().await;
    let html = commit_page(&f, None).await;
    // `+++`/`---` start with the add/del markers but are headers, never lines.
    assert!(html.contains(r#"<span class="diff-meta">--- a/notes</span>"#), "{html}");
    assert!(html.contains(r#"<span class="diff-meta">+++ b/notes</span>"#), "{html}");
    assert!(
        html.contains(r#"<span class="diff-hunk">@@ -1,3 +1,2 @@</span>"#),
        "{html}"
    );
    // No extension -> `language-plaintext`; the `+`/`-` marker sits outside
    // the `<code>`, since it isn't source text.
    assert!(
        html.contains(r#"<span class="diff-add-line">+<code class="language-plaintext">new one</code></span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="diff-del-line">-<code class="language-plaintext">old one</code></span>"#),
        "{html}"
    );
}

/// Each line is tagged with the language of the file it belongs to, which
/// means the path has to be tracked from each file's own headers: the `b/`
/// side normally, the `a/` side when the new one is `/dev/null`, never
/// `/dev/null` itself, and without the tab git appends to a spaced name.
#[tokio::test]
async fn each_diff_line_is_tagged_with_its_own_files_language() {
    let f = fixture().await;
    let html = commit_page(&f, None).await;
    for (marker, language, text) in [
        ('+', "rs", "added"),
        ('-', "go", "package gone"),
        ('+', "py", "print()"),
        ('+', "rs", "b"),
    ] {
        let line = format!(r#"{marker}<code class="language-{language}">{text}</code>"#);
        assert!(html.contains(&line), "{line}: {html}");
    }
    assert!(!html.contains("language-rs\t"), "{html}");
    assert!(!html.contains("language-dev"), "{html}");
}

#[tokio::test]
async fn side_by_side_pairs_dels_against_adds_row_by_row() {
    let f = fixture().await;
    let html = commit_page(&f, Some("diff-view=split")).await;
    // Each side carries its own line number, GitHub-style: "old one"/"new
    // one" land on the same row (positional pairing), "old two" has no
    // partner so the new side's cells are blank instead.
    for row in [
        r#"<td class="lineno">1</td><td class="diff-del-line"><code class="language-plaintext">old one</code></td><td class="lineno">1</td><td class="diff-add-line"><code class="language-plaintext">new one</code></td>"#,
        r#"<td class="lineno">2</td><td class="diff-del-line"><code class="language-plaintext">old two</code></td><td class="lineno"></td><td></td>"#,
        r#"<td class="lineno">3</td><td><code class="language-plaintext">context</code></td><td class="lineno">2</td><td><code class="language-plaintext">context</code></td>"#,
        // Numbered from where this file's hunk header starts, not from 1.
        r#"<td class="lineno">8</td><td class="diff-del-line"><code class="language-rs">gone</code></td><td class="lineno">8</td><td class="diff-add-line"><code class="language-rs">added</code></td>"#,
    ] {
        assert!(html.contains(row), "{row}: {html}");
    }
}

#[tokio::test]
async fn side_by_side_spans_hunk_and_file_headers_across_both_columns() {
    let f = fixture().await;
    let html = commit_page(&f, Some("diff-view=split")).await;
    for row in [
        r#"<td class="diff-meta" colspan="4">diff --git a/notes b/notes</td>"#,
        r#"<td class="diff-meta" colspan="4">--- a/notes</td>"#,
        r#"<td class="diff-meta" colspan="4">+++ b/notes</td>"#,
        r#"<td class="diff-hunk" colspan="4">@@ -1,3 +1,2 @@</td>"#,
    ] {
        assert!(html.contains(row), "{row}: {html}");
    }
}

#[tokio::test]
async fn recent_commits_carry_their_ref_decorations_as_badges() {
    let f = fixture().await;
    let html = get(&f.app, "/team/proj?view=tree", None).await;
    // `%D`'s ", "-separated list is one badge per ref, escaped.
    assert!(
        html.contains(r#"<td>rework <span class="badge accent">HEAD -&gt; main</span></td>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<td>before <span class="badge accent">tag: v1.0</span></td>"#),
        "{html}"
    );
    // No refs, no badge.
    assert!(html.contains("<td>start</td>"), "{html}");
}
