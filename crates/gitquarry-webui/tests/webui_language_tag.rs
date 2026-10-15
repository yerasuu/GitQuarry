//! The `language-*` class a blob's `<code>` carries, which is what
//! `highlight.js` tokenizes it by.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_test_support::{Work, ctx, send};
use std::sync::Arc;

/// `acme/proj` with one commit on `main` holding `files`, pushed the way
/// receive-pack lays it out: a pack plus the manifest pointing at it.
async fn app_with_files(root: &std::path::Path, files: &[&str]) -> axum::Router {
    let work = Work::new().await;
    let contents: Vec<_> = files.iter().map(|file| (*file, "x\n")).collect();
    work.commit(&contents, "files").await;

    let ctx = ctx(root);
    let repo = ctx.create_repo("acme/proj").await.unwrap().unwrap();
    work.push(&repo, "refs/heads/main").await;
    gitquarry_webui::webui::router(Arc::new(ctx))
}

async fn blob_code_class(app: &axum::Router, path: &str) -> String {
    let request = Request::get(format!("/acme/proj?view=blob&path={path}"))
        .body(Body::empty())
        .unwrap();
    let page = send(app, request).await;
    assert_eq!(page.status, StatusCode::OK, "{path}");
    let class = page.body.split(r#"<code id="blob-code" class=""#).nth(1).unwrap();
    class[..class.find('"').unwrap()].to_string()
}

#[tokio::test]
async fn blob_code_is_tagged_with_its_lowercased_extension() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_files(dir.path(), &["src/main.rs", "README.MD", "Dockerfile"]).await;

    assert_eq!(blob_code_class(&app, "src/main.rs").await, "language-rs");
    assert_eq!(blob_code_class(&app, "README.MD").await, "language-md");
    // No extension: `highlight.js`'s own built-in name for "don't tokenize".
    assert_eq!(blob_code_class(&app, "Dockerfile").await, "language-plaintext");
}

/// A pushed file name is attacker-chosen, and its extension lands in a
/// `class` attribute: one carrying a quote must not get to close it.
#[tokio::test]
async fn an_extension_outside_the_safe_charset_is_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_files(dir.path(), &[r#"x."onmouseover=alert(1)"#]).await;

    assert_eq!(
        blob_code_class(&app, "x.%22onmouseover%3Dalert(1)").await,
        "language-plaintext"
    );
}

/// Same file seen through a diff, where the path comes from the C-quoted
/// `+++` header rather than the tree.
#[tokio::test]
async fn a_diff_never_splices_a_quoted_extension_into_markup() {
    let dir = tempfile::tempdir().unwrap();
    let work = Work::new().await;
    let files = [(r#"x."onmouseover=alert(1)"#, "x\n"), ("ok.rs", "x\n")];
    let sha = work.commit(&files, "files").await;
    let ctx = ctx(dir.path());
    let repo = ctx.create_repo("acme/proj").await.unwrap().unwrap();
    work.push(&repo, "refs/heads/main").await;
    let app = gitquarry_webui::webui::router(Arc::new(ctx));

    let request = Request::get(format!("/acme/proj?view=commit&sha={sha}"))
        .body(Body::empty())
        .unwrap();
    let page = send(&app, request).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.body.contains("\"onmouseover"), "{}", page.body);
    assert!(page.body.contains(r#"class="language-rs""#), "{}", page.body);
}
