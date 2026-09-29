//! GitHub-style compound URLs (`/{repo}/blob/{branch}/{path}` and
//! `/_raw/plain/{repo}/refs/heads/{branch}/{path}`), where a branch name
//! may itself contain `/`: the split has to follow the refs that really
//! exist, never a fixed segment count.

use axum::Router;
use axum::http::StatusCode;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Session, Work, git};
use std::sync::Arc;

/// `team/proj` with `v1/f.txt` ("nested") and `f.txt` ("top") on `main`,
/// plus `branch` created off it.
async fn seeded(branch: &str) -> (tempfile::TempDir, Work, Router) {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(root.path()));
    gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    work.commit(&[("v1/f.txt", "nested"), ("f.txt", "top")], "init").await;
    git(work.path(), &["branch", branch]).await;
    let resolved = ctx.resolve_repo("team/proj").await.unwrap().unwrap();
    work.push(&resolved, "refs/heads/main").await;
    (root, work, gitquarry_webui::webui::router(ctx))
}

#[tokio::test]
async fn splits_branch_and_path_by_checking_real_refs() {
    // Branch "release" exists (not "release/v1" — git's ref namespace can't
    // hold both at once, same as it can't hold both a file and a directory
    // at one path): "release/v1/f.txt" must split as branch="release",
    // path="v1/f.txt".
    let (_root, _work, app) = seeded("release").await;

    let raw = Session::get_as(&app, "/_raw/plain/team/proj/refs/heads/release/v1/f.txt", None).await;
    assert_eq!(raw.status, StatusCode::OK);
    assert_eq!(raw.body, "nested");

    let html = Session::get_as(&app, "/team/proj/blob/release/v1/f.txt", None).await;
    assert_eq!(html.status, StatusCode::OK, "{}", html.body);
    assert!(html.body.contains("nested"), "{}", html.body);
}

#[tokio::test]
async fn falls_back_to_the_longer_ref_when_the_short_prefix_is_not_one() {
    // Only "release/v1" exists as a branch (no bare "release"), so the same
    // segments must split the other way: branch="release/v1", path="f.txt".
    let (_root, _work, app) = seeded("release/v1").await;

    let raw = Session::get_as(&app, "/_raw/plain/team/proj/refs/heads/release/v1/f.txt", None).await;
    assert_eq!(raw.status, StatusCode::OK);
    assert_eq!(raw.body, "top");

    let html = Session::get_as(&app, "/team/proj/blob/release/v1/f.txt", None).await;
    assert_eq!(html.status, StatusCode::OK, "{}", html.body);
    assert!(html.body.contains("top"), "{}", html.body);
    assert!(!html.body.contains("nested"), "{}", html.body);
}

#[tokio::test]
async fn a_private_repo_is_not_reachable_through_the_compound_form() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(root.path()));
    gitquarry_core::domain::repo::create(&ctx, "team/secret", None, None, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    work.commit(&[("f.txt", "classified")], "init").await;
    let resolved = ctx.resolve_repo("team/secret").await.unwrap().unwrap();
    work.push(&resolved, "refs/heads/main").await;
    let app = gitquarry_webui::webui::router(ctx);

    for uri in [
        "/team/secret/blob/main/f.txt",
        "/_raw/plain/team/secret/refs/heads/main/f.txt",
    ] {
        let page = Session::get_as(&app, uri, None).await;
        assert_eq!(page.status, StatusCode::NOT_FOUND, "GET {uri}");
        assert!(!page.body.contains("classified"), "GET {uri}: {}", page.body);
    }
}
