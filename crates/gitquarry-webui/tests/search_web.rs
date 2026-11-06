//! `/_search` end to end: what it scans (the stored index, else live git, and
//! peer mirrors already on disk), what it shows, and what it never does —
//! reach out to a peer on its own, or list a repo the viewer can't see.

use axum::body::Bytes;
use axum::http::StatusCode;
use gitquarry_core::app::{AppContext, FederationLink, ResolvedRepo};
use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use gitquarry_core::storage::search_index::{IndexedCommit, SearchIndex, put_search_index};
use gitquarry_test_support::{Session, Work, ctx};
use std::net::SocketAddr;
use std::sync::Arc;

/// The rendered `/_search` page for `query` (already URL-encoded).
async fn search(ctx: &Arc<AppContext>, query: &str) -> String {
    let app = gitquarry_webui::webui::router(Arc::clone(ctx));
    let page = Session::get_as(&app, &format!("/_search{query}"), None).await;
    assert_eq!(page.status, StatusCode::OK, "GET /_search{query}");
    page.body
}

/// A repo with `visibility`, listed in the visible index the way the
/// settings page leaves a public one.
async fn repo(ctx: &AppContext, name: &str, visibility: Visibility, display_name: Option<&str>) -> ResolvedRepo {
    let resolved = ctx.create_repo(name).await.unwrap().unwrap();
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: name.to_string(),
        visibility,
        display_name: display_name.map(str::to_string),
        ..Default::default()
    };
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();
    gitquarry_core::storage::settings::set_visible_index(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &resolved.uid,
        visibility != Visibility::Private,
    )
    .await;
    resolved
}

async fn plant_index(ctx: &AppContext, resolved: &ResolvedRepo, files: Vec<String>, commits: Vec<IndexedCommit>) {
    let index = SearchIndex {
        last_indexed_sha: Some("planted-sha".to_string()),
        files,
        commits,
    };
    put_search_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, &index, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_empty_query_renders_the_form_without_result_sections() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let html = search(&ctx, "").await;
    assert!(html.contains(r#"input name="q" value="""#), "{html}");
    assert!(!html.contains("<h4>"), "{html}");
}

#[tokio::test]
async fn the_query_is_echoed_back_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let html = search(&ctx, "?q=%22%3E%3Cscript%3Ehello").await;
    assert!(!html.contains("<script>hello"), "query rendered as markup");
    assert!(
        html.contains(r#"input name="q" value="&quot;&gt;&lt;script&gt;hello""#),
        "{html}"
    );
    assert!(html.contains("<h4>"), "a non-empty query renders its result sections");
}

/// The repo below has no commits at all (`create_repo` only claims the
/// name), so a live `git ls-tree`/`git log` against it finds nothing: the
/// only way these hits can appear is by reading the stored `SearchIndex`.
#[tokio::test]
async fn hits_come_from_the_stored_index_instead_of_live_git() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let resolved = repo(&ctx, "acme/my-proj", Visibility::Public, Some("My Proj!")).await;
    plant_index(
        &ctx,
        &resolved,
        vec!["planted.txt".to_string()],
        vec![IndexedCommit {
            sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            subject: "planted subject".to_string(),
        }],
    )
    .await;

    let html = search(&ctx, "?q=planted").await;
    assert!(html.contains(">planted.txt<"), "{html}");
    assert!(
        html.contains("planted-sha"),
        "the blob link should pin the indexed rev: {html}"
    );
    assert!(html.contains(">planted subject<"), "{html}");
    assert!(html.contains("0123456789abcdef0123456789abcdef01234567"), "{html}");
    // Rows are labelled with the repo's display name, linked by the path the
    // listing's slug backfill gives it.
    assert!(html.contains(r#"href="/acme/My-Proj""#), "{html}");
    assert!(html.contains("acme/My Proj!"), "{html}");
    assert!(!html.contains("showing the first 100 matches"), "{html}");
}

#[tokio::test]
async fn more_than_a_hundred_hits_are_cut_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let resolved = repo(&ctx, "acme/big", Visibility::Public, None).await;
    // `search::web::MAX_RESULTS`.
    plant_index(
        &ctx,
        &resolved,
        (0..101).map(|i| format!("hit-{i:03}.txt")).collect(),
        Vec::new(),
    )
    .await;
    let html = search(&ctx, "?q=hit-").await;
    assert!(html.contains("showing the first 100 matches"), "{html}");
    assert_eq!(html.matches(r#".txt</a>"#).count(), 100, "{html}");
}

/// Search reaches exactly what the index shows: a private repo is still
/// reachable by URL for someone with a grant, but never listed here.
#[tokio::test]
async fn a_private_repo_is_never_searched() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let resolved = repo(&ctx, "acme/secret", Visibility::Private, None).await;
    plant_index(&ctx, &resolved, vec!["planted.txt".to_string()], Vec::new()).await;
    let html = search(&ctx, "?q=planted").await;
    assert!(!html.contains("planted.txt"), "{html}");
    assert!(!html.contains("acme/secret"), "{html}");
}

/// A repo that resolves locally but has never been indexed falls back to
/// the live path instead of silently contributing zero hits forever.
#[tokio::test]
async fn a_repo_without_a_stored_index_falls_back_to_live_git() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(dir.path()));
    let resolved = repo(&ctx, "acme/demo", Visibility::Public, None).await;
    let work = Work::new().await;
    work.commit(&[("live.txt", "live.txt content\n")], "add live file")
        .await;
    work.push(&resolved, "refs/heads/main").await;

    let html = search(&ctx, "?q=live").await;
    assert!(html.contains(">live.txt<"), "{html}");
    assert!(html.contains(">add live file<"), "{html}");
}

async fn spawn_fake_peer_repos(reply: serde_json::Value) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = Bytes::from(serde_json::to_vec(&reply).unwrap());
    let app = axum::Router::new().route(
        "/_internal-out/peer-repos",
        axum::routing::get(move || async move { ([("content-type", "application/json")], body) }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn peer_metadata_hits_link_to_the_remote_browse_view() {
    let base_url = spawn_fake_peer_repos(serde_json::json!([
        {
            "peer_name": "instance-b",
            "repos": [
                { "name": "acme/other", "description": "a mirrored peer repo" },
                { "name": "acme/unrelated", "description": "nothing to see" },
            ],
        },
    ]))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(AppContext {
        federation: Some(FederationLink {
            base_url,
            internal_token: "tok".to_string(),
            http: reqwest::Client::new(),
        }),
        ..ctx(dir.path())
    });
    let html = search(&ctx, "?q=mirrored").await;
    assert!(html.contains(r#"href="/_remote/instance-b/acme/other""#), "{html}");
    assert!(html.contains("a mirrored peer repo"), "{html}");
    assert!(!html.contains("acme/unrelated"), "{html}");
}

/// Real socket end to end: a linked peer's repo is searchable once someone
/// has opened (and so mirrored) it, labelled as remote — and a search never
/// fetches from that peer by itself: what it finds is what's already on
/// disk, until something else syncs the mirror again.
#[tokio::test]
async fn a_peer_repo_is_searched_from_its_mirror_and_never_synced_by_search() {
    let dir_b = tempfile::tempdir().unwrap();
    let ctx_b = Arc::new(ctx(dir_b.path()));
    let resolved = repo(&ctx_b, "acme/demo", Visibility::Public, None).await;
    let work = Work::new().await;
    work.commit(&[("needle.txt", "needle.txt content\n")], "add needle")
        .await;
    work.push(&resolved, "refs/heads/main").await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer_url = format!("http://{}", listener.local_addr().unwrap());
    let app_b = gitquarry_webui::webui::router(Arc::clone(&ctx_b));
    tokio::spawn(async move {
        axum::serve(listener, app_b.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap()
    });

    let dir_a = tempfile::tempdir().unwrap();
    let ctx_a = Arc::new(ctx(dir_a.path()));

    // Not opened yet, so not a candidate — and searching doesn't make it one.
    let html = search(&ctx_a, "?q=needle").await;
    assert!(!html.contains("needle.txt"), "{html}");

    ctx_a
        .open_remote_mirror_at("instance-b", &peer_url, "acme/demo")
        .await
        .unwrap();
    let html = search(&ctx_a, "?q=needle").await;
    assert!(html.contains(">needle.txt<"), "{html}");
    assert!(html.contains(">add needle<"), "{html}");
    assert!(html.contains(r#"href="/_remote/instance-b/acme/demo""#), "{html}");
    assert!(html.contains("instance-b/acme/demo (remote)"), "{html}");

    work.commit(&[("fresh.txt", "fresh.txt content\n")], "add fresh").await;
    work.push(&resolved, "refs/heads/main").await;
    let html = search(&ctx_a, "?q=fresh").await;
    assert!(!html.contains("fresh.txt"), "search fetched from the peer: {html}");

    ctx_a
        .open_remote_mirror_at("instance-b", &peer_url, "acme/demo")
        .await
        .unwrap();
    let html = search(&ctx_a, "?q=fresh").await;
    assert!(
        html.contains(">fresh.txt<"),
        "the resynced mirror should now match: {html}"
    );
}
