//! Every browsing view and raw export of a repo, driven through `router()`
//! against real content pushed into the bucket: the parser unit tests in
//! `git::browse` cover the string-munging in isolation, but only this would
//! catch a mismatch against actual `git` output (wrong flag, wrong field
//! order, a mirror sync that never landed the pushed objects).

use axum::Router;
use axum::http::{StatusCode, header};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Page, Session, Work, ctx, git};
use std::sync::Arc;

struct Seeded {
    _dirs: (tempfile::TempDir, Work),
    app: Router,
    first_sha: String,
}

/// `team/proj`, public: an initial commit (README + `src.rs`) with a
/// `feature` branch and an annotated `v1.0` tag on it, then a second commit
/// on `main` that edits `src.rs`.
async fn seeded() -> Seeded {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let resolved = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let work = Work::new().await;
    let first_sha = work
        .commit(
            &[
                ("README.md", "# Hello\n\nSome **bold** text.\n"),
                ("src.rs", "fn main() {}\n"),
            ],
            "initial commit",
        )
        .await;
    git(work.path(), &["branch", "feature"]).await;
    git(work.path(), &["tag", "-a", "v1.0", "-m", "release notes"]).await;
    work.commit(
        &[("src.rs", "fn main() {\n    println!(\"hi\");\n}\n")],
        "add a println",
    )
    .await;
    work.push(&resolved, "refs/heads/main").await;

    Seeded {
        _dirs: (root, work),
        app: gitquarry_webui::webui::router(ctx),
        first_sha,
    }
}

async fn page(app: &Router, uri: &str) -> String {
    let Page { status, body, .. } = Session::get_as(app, uri, None).await;
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
    let Page { body: split, .. } = Session::get_as(
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

    let Page { status, headers, body } = Session::get_as(&s.app, "/_raw/plain/team/proj?path=src.rs", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    assert_eq!(body, "fn main() {\n    println!(\"hi\");\n}\n");

    // A missing path errors plain-text, not the normal HTML error page — a
    // raw view can't leak markup into a client expecting raw bytes.
    let Page { status, headers, .. } =
        Session::get_as(&s.app, "/_raw/plain/team/proj?path=no-such-file.rs", None).await;
    assert!(status.is_client_error() || status.is_server_error(), "{status}");
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");

    let Page {
        status, body: patch, ..
    } = Session::get_as(&s.app, &format!("/_raw/patch/team/proj?sha={}", s.first_sha), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(patch.contains("initial commit"), "patch: {patch}");

    let Page { headers, .. } = Session::get_as(&s.app, "/_raw/snapshot/team/proj?fmt=zip", None).await;
    assert_eq!(headers[header::CONTENT_TYPE], "application/zip");

    let Page { headers, .. } = Session::get_as(&s.app, "/_raw/snapshot/team/proj?fmt=tar.gz", None).await;
    assert_eq!(headers[header::CONTENT_TYPE], "application/gzip");

    let Page { headers, .. } = Session::get_as(&s.app, "/_raw/atom/team/proj", None).await;
    assert_eq!(headers[header::CONTENT_TYPE], "application/atom+xml; charset=utf-8");
}
