//! The `/_raw/...` links pages hand out: each one a browser can follow
//! straight to the bytes it names, so the link builder and the route table
//! are checked against each other rather than against a hardcoded string.

use axum::Router;
use axum::http::{StatusCode, header};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::{AssetMeta, ReleaseMeta};
use gitquarry_test_support::{Page, Session, Work, ctx, git};
use std::sync::Arc;

/// `team/proj` with `src/lib.rs` on `main`, tag `v1.0`, and a release on
/// it carrying one uploaded asset, `proj.zip`.
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
    git(work.path(), &["tag", "v1.0"]).await;
    work.push(&resolved, "refs/heads/main").await;

    let asset = work.path().join("proj.zip");
    std::fs::write(&asset, b"asset bytes").unwrap();
    resolved.remote.upload_asset("v1.0", "proj.zip", &asset).await.unwrap();
    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "First release".to_string(),
        notes: String::new(),
        assets: vec![AssetMeta {
            filename: "proj.zip".to_string(),
            size: 11,
            sha256: "a".repeat(64),
            uploaded_at: "2024-01-01T00:00:00Z".to_string(),
            uploaded_by: "alice".to_string(),
            signature: None,
            signature_verified: false,
        }],
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
async fn the_blob_page_links_its_github_style_raw_url() {
    let (_root, _work, app, _) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, "/team/proj?view=blob&path=src/lib.rs", None).await;
    let link = href(&html, "/_raw/plain/");
    assert_eq!(link, "/_raw/plain/team/proj/refs/heads/main/src/lib.rs");

    let Page { status, headers, body } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    assert_eq!(
        body,
        "pub fn answer() -> u32 { 42 }
"
    );
}

#[tokio::test]
async fn the_atom_link_carries_no_empty_query_fields() {
    let (_root, _work, app, _) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, "/team/proj", None).await;
    let link = href(&html, "/_raw/atom/");
    assert_eq!(link, "/_raw/atom/team/proj");

    let Page { status, headers, body } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/atom+xml; charset=utf-8");
    assert!(body.contains("initial commit"), "{body}");
}

#[tokio::test]
async fn the_commit_page_links_its_patch() {
    let (_root, _work, app, sha) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, &format!("/team/proj?view=commit&sha={sha}"), None).await;
    let link = href(&html, "/_raw/patch/");
    assert_eq!(link, format!("/_raw/patch/team/proj?sha={sha}"));

    let Page { status, body, .. } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("initial commit"), "{body}");
}

#[tokio::test]
async fn the_release_page_links_each_asset_download() {
    let (_root, _work, app, _) = seeded().await;
    let Page { body: html, .. } = Session::get_as(&app, "/team/proj?view=release&tag=v1.0", None).await;
    let link = href(&html, "/_raw/release_asset/");
    assert_eq!(link, "/_raw/release_asset/team/proj?tag=v1.0&filename=proj.zip");

    let Page { status, headers, body } = Session::get_as(&app, &link, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment"),
        "{headers:?}"
    );
    assert_eq!(body, "asset bytes");
}
