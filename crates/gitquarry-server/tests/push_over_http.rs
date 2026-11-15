//! A real `git push` over smart-HTTP into a server whose backend is the one
//! `gitquarry.toml` builds (cache included), with the real `gitquarry
//! pre-receive-hook` writing the bucket from its own process — the only
//! setup where this process's cached `refs.json` and the bucket's disagree.

use gitquarry_core::app::AppContext;
use gitquarry_core::config::Config;
use gitquarry_core::crypto::bootstrap;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{PASSWORD, Work, git};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

const PASSPHRASE: &str = "hunter2";

/// The `files` engine, not `redb`: the hook opens the bucket while the
/// server still has it open, and redb locks its file to one process.
async fn server_ctx(root: &Path) -> AppContext {
    let bucket = root.join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let config_path = root.join("gitquarry.toml");
    std::fs::write(
        &config_path,
        format!(
            "[buckets.main]\ndriver = \"local\"\nengine = \"files\"\nbucket = {:?}\npassphrase = {PASSPHRASE:?}\n",
            bucket.to_string_lossy()
        ),
    )
    .unwrap();
    let config = Config::load_from(&config_path).unwrap();
    let backend = gitquarry_core::storage::build_backend(config.bucket("main").unwrap())
        .unwrap()
        .backend;
    let cipher = Arc::new(
        bootstrap::get_or_create_cipher(backend.as_ref(), PASSPHRASE)
            .await
            .unwrap(),
    );
    AppContext {
        bucket: "main".to_string(),
        config_path,
        exe_path: env!("CARGO_BIN_EXE_gitquarry").into(),
        backend,
        cipher,
        ..gitquarry_test_support::ctx(root)
    }
}

async fn serve(ctx: Arc<AppContext>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND })
        .layer(axum::middleware::from_fn_with_state(
            ctx,
            gitquarry_core::git::http::middleware,
        ));
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    format!("http://alice:{}@{addr}", PASSWORD.replace(' ', "%20"))
}

/// A repo made the way `/_new` makes one, owned by alice, and its HTTP URL.
async fn new_repo(root: &Path) -> (Arc<AppContext>, String) {
    let ctx = server_ctx(root).await;
    gitquarry_test_support::user(&ctx, "alice", false).await;
    let owner = gitquarry_core::domain::repo::resolve_owner_ref(&ctx, "alice")
        .await
        .unwrap();
    gitquarry_core::domain::repo::create(&ctx, "alice/proj", None, owner, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap();
    let ctx = Arc::new(ctx);
    let base = serve(Arc::clone(&ctx)).await;
    (ctx, format!("{base}/alice/proj"))
}

#[tokio::test]
async fn the_only_branch_pushed_into_a_new_repo_is_its_head_whatever_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, url) = new_repo(dir.path()).await;
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n")], "first").await;
    git(work.path(), &["branch", "-q", "-m", "trunk"]).await;

    git(work.path(), &["push", "-q", &url, "trunk"]).await;

    let resolved = ctx.resolve_repo("alice/proj").await.unwrap().unwrap();
    let mirror = ctx.open_mirror_read(&resolved).await.unwrap();
    assert_eq!(
        gitquarry_core::git::browse::resolve_head(&mirror.path)
            .await
            .unwrap()
            .as_deref(),
        Some("refs/heads/trunk")
    );
}

/// A repo with `trunk` (HEAD) and `side` pushed, and a work tree holding both.
async fn repo_with_trunk_and_side(dir: &Path) -> (Arc<AppContext>, String, Work) {
    let (ctx, url) = new_repo(dir).await;
    let work = Work::new().await;
    work.commit(&[("a.txt", "one\n")], "first").await;
    git(work.path(), &["branch", "-q", "-m", "trunk"]).await;
    git(work.path(), &["push", "-q", &url, "trunk"]).await;
    git(work.path(), &["branch", "-q", "side", "trunk"]).await;
    git(work.path(), &["push", "-q", &url, "side"]).await;
    (ctx, url, work)
}

/// Read off the mirror a fresh sync builds from the bucket's manifest.
async fn branches_in_bucket(ctx: &AppContext) -> (Option<String>, Vec<String>) {
    let resolved = ctx.resolve_repo("alice/proj").await.unwrap().unwrap();
    let mirror = ctx.open_mirror_read(&resolved).await.unwrap();
    let head = gitquarry_core::git::browse::resolve_head(&mirror.path).await.unwrap();
    let refs = git(&mirror.path, &["for-each-ref", "--format=%(refname)", "refs/heads"]).await;
    (head, refs.lines().map(str::to_owned).collect())
}

#[tokio::test]
async fn a_push_deleting_the_head_branch_is_rejected_and_the_bucket_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, url, work) = repo_with_trunk_and_side(dir.path()).await;

    let out = tokio::process::Command::new("git")
        .current_dir(work.path())
        .args(["push", "-q", &url, ":trunk"])
        .output()
        .await
        .unwrap();

    assert!(!out.status.success());
    let (head, refs) = branches_in_bucket(&ctx).await;
    assert_eq!(head.as_deref(), Some("refs/heads/trunk"));
    assert_eq!(refs, ["refs/heads/side", "refs/heads/trunk"]);
}

#[tokio::test]
async fn a_push_deleting_a_branch_other_than_head_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, url, work) = repo_with_trunk_and_side(dir.path()).await;

    git(work.path(), &["push", "-q", &url, ":side"]).await;

    let (head, refs) = branches_in_bucket(&ctx).await;
    assert_eq!(head.as_deref(), Some("refs/heads/trunk"));
    assert_eq!(refs, ["refs/heads/trunk"]);
}
