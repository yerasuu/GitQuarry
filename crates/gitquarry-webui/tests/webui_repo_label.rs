//! The label a repo is listed under, as opposed to the slug it's addressed
//! by: `display_name` replaces only the local segment, and the href next to
//! it keeps the slug — the only string that routes.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[25u8; s3db_crypto::SALT_LEN]).unwrap());
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

async fn get(ctx: &Arc<AppContext>, uri: &str) -> String {
    let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = gitquarry_webui::webui::router(Arc::clone(ctx))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let page = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(status, StatusCode::OK, "GET {uri}: {page}");
    page
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
    let ctx = Arc::new(test_ctx(root.path()));
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
    let ctx = Arc::new(test_ctx(root.path()));
    create(&ctx, "acme/my-proj", Some("My Proj!")).await;
    create(&ctx, "acme/plain", None).await;

    assert!(get(&ctx, "/acme/my-proj").await.contains("<title>acme/My Proj!"));
    assert!(get(&ctx, "/acme/plain").await.contains("<title>acme/plain"));
}
