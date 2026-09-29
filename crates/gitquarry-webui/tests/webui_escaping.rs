use axum::http::StatusCode;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Session, Work, ctx};
use gitquarry_webui::webui::esc;
use std::sync::Arc;

#[test]
fn escapes_html_metacharacters() {
    assert_eq!(esc("<script>&\"'"), "&lt;script&gt;&amp;&quot;&#39;");
}

/// A tree entry's name reaches its href percent-encoded — everything
/// outside RFC 3986's unreserved set, but never the `/` between repo path
/// segments — and the encoded link routes back to the same file.
#[tokio::test]
async fn tree_links_percent_encode_names_but_keep_path_separators() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/project", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    work.commit(
        &[("a b?c.txt", "spaced out\n"), ("my dir/inner.txt", "inner\n")],
        "names that need encoding",
    )
    .await;
    work.push(&repo, "refs/heads/main").await;
    let app = gitquarry_webui::webui::router(ctx);

    let tree = Session::get_as(&app, "/team/project?view=tree", None).await;
    assert_eq!(tree.status, StatusCode::OK, "{}", tree.body);
    let tree = tree.body;
    assert!(
        tree.contains(r#"href="/team/project/blob/main/a%20b%3Fc.txt""#),
        "{tree}"
    );
    assert!(
        tree.contains(r#"href="/team/project?view=tree&amp;ref=main&amp;path=my%20dir""#),
        "{tree}"
    );

    let blob = Session::get_as(&app, "/team/project/blob/main/a%20b%3Fc.txt", None).await;
    assert_eq!(blob.status, StatusCode::OK, "{}", blob.body);
    let blob = blob.body;
    assert!(blob.contains("spaced out"), "{blob}");
}
