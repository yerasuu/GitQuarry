use gitquarry_core::app::AppContext;
use gitquarry_core::git;
use gitquarry_webui::webui::export;
use gitquarry_webui::webui::repo::summary::render_readme;
use gitquarry_webui::webui::repo::*;
use std::sync::Arc;

/// Minimal `AppContext` so `render_refs` can run its write-permission
/// check — same shape as `releases::tests::test_ctx`, duplicated for the
/// same reason (sibling test modules, no natural common parent).
fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Files,
        }),
    );
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
    );
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
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        events: Default::default(),
    }
}

/// Builds a real (non-bare) repo in a tempdir and drives the render
/// functions against it with real `git` subprocesses — the parser unit
/// tests in `git::browse` cover the string-munging in isolation, but
/// this is the only thing that would catch a mismatch against actual
/// `git` output (wrong flag, wrong field order, etc). `git_dir` works
/// the same for a non-bare repo's `.git` as for a bare mirror: none of
/// `gitquarry_core::git`'s commands touch the working tree.
#[tokio::test]
async fn renders_full_repo_views_against_real_git() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    let git_dir = work.join(".git");

    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(work)
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };

    run(&["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("README.md"), "# Hello\n\nSome **bold** text.\n").unwrap();
    std::fs::write(work.join("src.rs"), "fn main() {}\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "--quiet", "-m", "initial commit"]);
    run(&["branch", "feature"]);
    run(&["tag", "-a", "v1.0", "-m", "release notes"]);

    let head = git::browse::resolve_head(&git_dir).await.unwrap().unwrap();
    assert_eq!(head, "refs/heads/main");

    let ctx = test_ctx(dir.path());

    let tree = render_tree(&ctx, &git_dir, "repo", "main", "", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(tree.contains("README.md"), "tree: {tree}");
    assert!(tree.contains("src.rs"), "tree: {tree}");

    let blob = render_blob(&git_dir, "repo", "main", "src.rs", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(blob.contains("fn"), "blob: {blob}");

    let log = render_log(&ctx, &git_dir, "repo", "main", "", None, 0, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(log.contains("initial commit"), "log: {log}");
    let refs = render_refs(
        &ctx,
        &git_dir,
        "repo",
        &axum::http::HeaderMap::new(),
        None,
        None,
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(refs.contains("feature"), "refs: {refs}");
    assert!(refs.contains("v1.0"), "refs: {refs}");
    assert!(refs.contains("release notes"), "refs: {refs}");

    let stats = render_stats(&git_dir, "main", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(stats.contains("Test Author"), "stats: {stats}");

    let commits = git::browse::log(&git_dir, "main", 1, None).await.unwrap();
    let sha = commits[0].sha.clone();
    let (info, diff) = git::browse::show_commit(&git_dir, &sha).await.unwrap();
    let commit_html = render_commit(&ctx, &git_dir, "repo", &info, &diff, &axum::http::HeaderMap::new())
        .await
        .unwrap();
    assert!(commit_html.contains("initial commit"), "commit: {commit_html}");
    // Split view toggled on: a real `git show` diff, laid out side by side.
    assert!(
        commit_html.contains(r#"value="split""#),
        "commit missing diff-view toggle: {commit_html}"
    );

    std::fs::write(work.join("src.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    run(&["commit", "--quiet", "-am", "add a println"]);
    let (info2, diff2) = git::browse::show_commit(&git_dir, "main").await.unwrap();
    let mut split_cookie = axum::http::HeaderMap::new();
    split_cookie.insert(axum::http::header::COOKIE, "diff-view=split".parse().unwrap());
    let split_html = render_commit(&ctx, &git_dir, "repo", &info2, &diff2, &split_cookie)
        .await
        .unwrap();
    assert!(split_html.contains("diff-split"), "split: {split_html}");
    assert!(split_html.contains("println"), "split: {split_html}");
    assert!(split_html.contains("size-btn active"), "split: {split_html}");

    let blame = render_blame(&git_dir, "repo", "main", "src.rs", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(blame.contains("Test Author"), "blame: {blame}");
    assert!(blame.contains("fn main"), "blame: {blame}");

    let readme = render_readme(&git_dir, "main").await.unwrap().unwrap();
    assert!(readme.contains("<strong>bold</strong>"), "readme: {readme}");

    // Non-HTML export views: git archive/format-patch/gzip actually run.
    let plain_resp = export::plain(&git_dir, "main", "src.rs", gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    assert_eq!(plain_resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        plain_resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );

    // A missing path errors plain-text, not the normal HTML error page —
    // a raw view can't leak markup into a client expecting raw bytes.
    let missing = export::plain(&git_dir, "main", "no-such-file.rs", gitquarry_i18n::Locale::Es)
        .await
        .unwrap_err()
        .into_plain_response();
    assert!(missing.status().is_server_error() || missing.status().is_client_error());
    assert_eq!(
        missing.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );

    let patch_resp = export::patch(&git_dir, &sha).await.unwrap();
    assert_eq!(patch_resp.status(), axum::http::StatusCode::OK);

    let zip_resp = export::snapshot(&git_dir, "repo", "main", "zip", gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    assert_eq!(
        zip_resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/zip"
    );

    let targz_resp = export::snapshot(&git_dir, "repo", "main", "tar.gz", gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    assert_eq!(
        targz_resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/gzip"
    );

    let atom_resp = export::atom(&git_dir, "repo", "main").await.unwrap();
    assert_eq!(
        atom_resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/atom+xml; charset=utf-8"
    );
}
