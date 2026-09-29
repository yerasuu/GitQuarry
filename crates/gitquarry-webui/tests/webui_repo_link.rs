//! The in-repo links pages hand out (`?view=` query strings and GitHub-style
//! `/blob/` paths): each checked for its exact shape and then followed, so
//! the link builder and the route/view dispatch are checked against each
//! other.

use axum::Router;
use axum::http::StatusCode;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::ReleaseMeta;
use gitquarry_test_support::{Page, Session, Work, ctx, git};
use std::sync::Arc;

/// `team/proj` with `src/lib.rs` on `main`, a `feature` branch, tag `v1.0`,
/// and a release on it.
async fn seeded() -> (tempfile::TempDir, Work, Router, String) {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let resolved = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    let sha = work
        .commit(&[("src/lib.rs", "pub fn answer() -> u32 { 42 }\n")], "initial commit")
        .await;
    git(work.path(), &["branch", "feature"]).await;
    git(work.path(), &["tag", "v1.0"]).await;
    work.push(&resolved, "refs/heads/main").await;

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

#[tokio::test]
async fn the_tree_links_a_file_by_its_github_style_blob_path() {
    let (_root, _work, app, _) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, "/team/proj?view=tree&path=src", None).await;
    let link = href(&html, "/team/proj/blob/");
    assert_eq!(link, "/team/proj/blob/main/src/lib.rs");

    let Page { status, body, .. } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("answer"), "{body}");
}

#[tokio::test]
async fn the_log_links_each_commit_by_sha() {
    let (_root, _work, app, sha) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, "/team/proj?view=log", None).await;
    let link = href(&html, "/team/proj?view=commit");
    assert_eq!(link, format!("/team/proj?view=commit&sha={sha}"));

    let Page { status, body, .. } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("initial commit"), "{body}");
}

#[tokio::test]
async fn the_refs_page_links_a_branch_tree_without_empty_fields() {
    let (_root, _work, app, _) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, "/team/proj?view=refs", None).await;
    let link = href(&html, "/team/proj?view=tree&amp;ref=feature");
    assert_eq!(link, "/team/proj?view=tree&ref=feature");

    let Page { status, body, .. } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("src"), "{body}");
}

#[tokio::test]
async fn the_releases_list_links_each_release_by_tag() {
    let (_root, _work, app, _) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, "/team/proj?view=releases", None).await;
    let link = href(&html, "/team/proj?view=release&amp;tag=");
    assert_eq!(link, "/team/proj?view=release&tag=v1.0");

    let Page { status, body, .. } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("First release"), "{body}");
}
