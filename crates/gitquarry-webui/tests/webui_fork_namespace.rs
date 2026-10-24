//! `POST /_fork/<repo>`, driven through `webui::router`: how a bad
//! destination namespace is told apart from a storage failure while
//! resolving it.

use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::{Backend, Cas};
use gitquarry_test_support::Session;
use s3db::{DeleteOutcome, ListPage};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The real backend, except that reading a group object fails once armed:
/// the one read `resolve_dest_owner` makes that a request's login and
/// source-repo checks don't.
#[derive(Debug)]
struct FlakyGroups {
    inner: Arc<dyn Backend>,
    armed: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Backend for FlakyGroups {
    async fn get(&self, key: &str) -> anyhow::Result<(Vec<u8>, Option<String>)> {
        if self.armed.load(Ordering::SeqCst) && key.starts_with(".conf/groups/") {
            anyhow::bail!("injected storage failure reading {key}");
        }
        self.inner.get(key).await
    }

    async fn put_cas(&self, key: &str, body: Vec<u8>, expected_etag: Option<&str>) -> anyhow::Result<Cas> {
        self.inner.put_cas(key, body, expected_etag).await
    }

    async fn delete(&self, key: &str) -> anyhow::Result<DeleteOutcome> {
        self.inner.delete(key).await
    }

    async fn delete_many(&self, keys: &[String]) -> anyhow::Result<DeleteOutcome> {
        self.inner.delete_many(keys).await
    }

    async fn list_prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, u64)>> {
        self.inner.list_prefix(prefix).await
    }

    async fn get_range(&self, key: &str, offset: u64, len: u64) -> anyhow::Result<Vec<u8>> {
        self.inner.get_range(key, offset, len).await
    }

    async fn list_page(&self, prefix: &str, after: Option<&str>, limit: usize) -> anyhow::Result<ListPage> {
        self.inner.list_page(prefix, after, limit).await
    }

    async fn put_stream(&self, key: &str, source: &std::path::Path) -> anyhow::Result<()> {
        self.inner.put_stream(key, source).await
    }

    async fn get_stream(&self, key: &str, dest: &std::path::Path) -> anyhow::Result<()> {
        self.inner.get_stream(key, dest).await
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct Instance {
    _dir: tempfile::TempDir,
    armed: Arc<AtomicBool>,
    app: axum::Router,
}

impl Instance {
    /// An instance with a user `alice` owning the public repo `alice/web`.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let base = gitquarry_test_support::ctx(dir.path());
        let ctx = Arc::new(AppContext {
            backend: Arc::new(FlakyGroups {
                inner: Arc::clone(&base.backend),
                armed: Arc::clone(&armed),
            }),
            ..base
        });
        gitquarry_test_support::user(&ctx, "alice", false).await;
        gitquarry_core::domain::repo::create(&ctx, "alice/web", None, None, None, Visibility::Public)
            .await
            .unwrap()
            .unwrap();
        let app = gitquarry_webui::webui::router(ctx);
        Self { _dir: dir, armed, app }
    }

    async fn fork_into(&self, namespace: &str, group_reads_fail: bool) -> gitquarry_test_support::Page {
        let alice = Session::login(&self.app, "alice").await;
        self.armed.store(group_reads_fail, Ordering::SeqCst);
        alice
            .post(
                &self.app,
                "/_fork/alice/web",
                &[("namespace", namespace), ("name", "web2")],
            )
            .await
    }
}

#[tokio::test]
async fn unknown_namespace_is_a_form_error() {
    let gq = Instance::new().await;
    let page = gq.fork_into("nowhere", false).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(page.body.contains("nowhere"), "{}", page.body);
}

#[tokio::test]
async fn storage_failure_resolving_the_namespace_is_a_server_error() {
    let gq = Instance::new().await;
    let page = gq.fork_into("nowhere", true).await;
    assert_eq!(page.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", page.body);
}
