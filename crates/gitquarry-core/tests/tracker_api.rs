//! Drives `tracker_api::run_server` over a real socket, token and wire
//! version included, the way `gitquarry-tracker`'s `CoreClient` calls it.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::entities::User;
use gitquarry_core::storage;
use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use reqwest::StatusCode;
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "tracker-test-token";

struct Server {
    ctx: Arc<AppContext>,
    base: String,
    _bucket: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        let bucket = tempfile::tempdir().unwrap();
        let ctx = Arc::new(gitquarry_test_support::ctx(bucket.path()));
        // run_server only takes an address, so a free port is found by
        // binding and releasing it first.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let server_ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            gitquarry_core::tracker_api::run_server(&format!("127.0.0.1:{port}"), server_ctx, TOKEN.to_string())
                .await
                .unwrap();
        });
        for _ in 0..100 {
            if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Server {
            ctx,
            base: format!("http://127.0.0.1:{port}/_internal"),
            _bucket: bucket,
        }
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> (StatusCode, serde_json::Value) {
        let mut url = reqwest::Url::parse(&format!("{}{path}", self.base)).unwrap();
        url.query_pairs_mut().extend_pairs(query);
        let resp = reqwest::Client::new().get(url).signed(TOKEN).send().await.unwrap();
        let status = resp.status();
        (status, resp.json().await.unwrap())
    }

    async fn corrupt(&self, key: &str) {
        self.ctx
            .backend()
            .put_cas(key, b"not a sealed object".to_vec(), None)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn labels_of_an_existing_repo_are_listed() {
    let server = Server::start().await;
    let repo = server.ctx.create_repo("alice/proj").await.unwrap().unwrap();
    let (mut config, etag) = repo.remote.get_repo_config().await.unwrap();
    config.full_name = "alice/proj".to_string();
    storage::settings::put_repo_config(
        server.ctx.backend().as_ref(),
        repo.uid.as_str(),
        &config,
        etag.as_deref(),
        &server.ctx.cipher,
    )
    .await
    .unwrap();
    let (status, json) = server.get("/repos/labels", &[("uid", repo.uid.as_str())]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["labels"].is_array());
}

#[tokio::test]
async fn labels_of_a_missing_repo_are_a_404() {
    let server = Server::start().await;
    let (status, _) = server.get("/repos/labels", &[("uid", "nope")]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A config that no longer opens must not read as "no such repo": tracker
/// would then reject every label as unknown and hide the fault.
#[tokio::test]
async fn labels_of_a_repo_whose_config_does_not_decrypt_are_a_500() {
    let server = Server::start().await;
    server.corrupt(&RepoConfig::COLLECTION.key("broken-uid").unwrap()).await;
    let (status, _) = server.get("/repos/labels", &[("uid", "broken-uid")]).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn user_exists_tells_a_real_user_from_a_missing_one() {
    let server = Server::start().await;
    gitquarry_test_support::account(&server.ctx, "alice", None).await;
    let (status, json) = server.get("/users/exists", &[("username", "alice")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["exists"], true);
    let (status, json) = server.get("/users/exists", &[("username", "bob")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["exists"], false);
}

#[tokio::test]
async fn user_exists_for_a_user_that_does_not_decrypt_is_a_500() {
    let server = Server::start().await;
    server.corrupt(&User::COLLECTION.key("mallory").unwrap()).await;
    let (status, _) = server.get("/users/exists", &[("username", "mallory")]).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}
