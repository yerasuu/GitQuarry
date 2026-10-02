//! The label a repo is listed under, as opposed to the slug it's addressed
//! by: `display_name` replaces only the local segment, and the href next to
//! it keeps the slug — the only string that routes.

use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Session, ctx};
use std::sync::Arc;

async fn get(ctx: &Arc<AppContext>, uri: &str) -> String {
    let app = gitquarry_webui::webui::router(Arc::clone(ctx));
    let page = Session::get_as(&app, uri, None).await;
    assert_eq!(page.status, StatusCode::OK, "GET {uri}: {}", page.body);
    page.body
}

async fn create(ctx: &AppContext, full_name: &str, display_name: Option<&str>) {
    gitquarry_core::domain::repo::create(
        ctx,
        full_name,
        display_name.map(str::to_string),
        None,
        None,
        Visibility::Public,
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn the_index_lists_each_repo_under_its_label_and_links_its_slug() {
    // Each slug is already what its display name slugifies to, since a
    // listing otherwise renames a drifted slug (`domain::repo::backfill_slug`)
    // and the hrefs below would follow the rename.
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    create(&ctx, "acme/My-Proj", Some("My Proj!")).await;
    create(&ctx, "acme/plain", None).await;
    create(&ctx, "acme/blank", Some("")).await;
    create(&ctx, "Solo-Proj", Some("Solo Proj")).await;
    create(&ctx, "acme/a-b", Some("a/b")).await;
    create(&ctx, "acme/b-bold-b", Some("<b>bold</b>")).await;

    let index = get(&ctx, "/").await;

    // Only the local segment is swapped; the namespace is the owner's.
    assert!(
        index.contains(r#"<a href="/acme/My-Proj">acme/My Proj!</a>"#),
        "{index}"
    );
    // No display name, or an empty one, falls back to the slug — never to
    // a blank label.
    assert!(index.contains(r#"<a href="/acme/plain">acme/plain</a>"#), "{index}");
    assert!(index.contains(r#"<a href="/acme/blank">acme/blank</a>"#), "{index}");
    // A namespaceless name is replaced whole.
    assert!(index.contains(r#"<a href="/Solo-Proj">Solo Proj</a>"#), "{index}");
    // A slash in the display name stays inside its own namespace.
    assert!(index.contains(r#"<a href="/acme/a-b">acme/a/b</a>"#), "{index}");
    // A display name is free text its owner typed.
    assert!(
        index.contains(r#"<a href="/acme/b-bold-b">acme/&lt;b&gt;bold&lt;/b&gt;</a>"#),
        "{index}"
    );
}

#[tokio::test]
async fn a_repo_page_is_titled_by_its_label() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    create(&ctx, "acme/my-proj", Some("My Proj!")).await;
    create(&ctx, "acme/plain", None).await;

    assert!(get(&ctx, "/acme/my-proj").await.contains("<title>acme/My Proj!"));
    assert!(get(&ctx, "/acme/plain").await.contains("<title>acme/plain"));
}
