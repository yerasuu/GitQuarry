use gitquarry_core::app::*;
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::Cas;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn locks_out_after_max_attempts_and_clear_resets_it() {
    let attempts = LoginAttempts::default();
    for _ in 0..LOGIN_MAX_ATTEMPTS {
        assert!(!attempts.is_locked("alice"));
        attempts.record_failure("alice");
    }
    assert!(attempts.is_locked("alice"));
    // A different username is unaffected.
    assert!(!attempts.is_locked("bob"));

    attempts.clear("alice");
    assert!(!attempts.is_locked("alice"));
}

#[test]
fn locked_count_reflects_only_usernames_actually_over_the_cap() {
    let attempts = LoginAttempts::default();
    assert_eq!(attempts.locked_count(), 0);

    // One failure short of the cap: not locked yet, doesn't count.
    for _ in 0..LOGIN_MAX_ATTEMPTS - 1 {
        attempts.record_failure("alice");
    }
    assert_eq!(attempts.locked_count(), 0);

    attempts.record_failure("alice");
    assert_eq!(attempts.locked_count(), 1);

    for _ in 0..LOGIN_MAX_ATTEMPTS {
        attempts.record_failure("bob");
    }
    assert_eq!(attempts.locked_count(), 2);

    attempts.clear("alice");
    assert_eq!(attempts.locked_count(), 1);
}

#[test]
fn anonymous_bucket_locks_out_an_ip_after_the_cap_and_a_different_ip_is_unaffected() {
    let limiter = GitRateLimiter::default();
    for _ in 0..GIT_RATE_LIMIT_ANONYMOUS_MAX {
        assert!(limiter.check_anonymous("1.2.3.4"));
    }
    assert!(!limiter.check_anonymous("1.2.3.4"));
    assert!(limiter.check_anonymous("5.6.7.8"));
}

#[test]
fn authenticated_bucket_is_independent_of_the_anonymous_one_and_has_a_higher_cap() {
    let limiter = GitRateLimiter::default();
    for _ in 0..GIT_RATE_LIMIT_ANONYMOUS_MAX {
        assert!(limiter.check_anonymous("1.2.3.4"));
    }
    // Same key string reused as a username in the other bucket: still
    // fresh, since the two buckets never share state. One call already
    // used, so the loop only needs to spend the rest of the cap.
    assert!(limiter.check_authenticated("1.2.3.4"));
    for _ in 0..GIT_RATE_LIMIT_AUTHENTICATED_MAX - 1 {
        assert!(limiter.check_authenticated("1.2.3.4"));
    }
    assert!(!limiter.check_authenticated("1.2.3.4"));
}

#[test]
fn limited_count_reflects_only_keys_actually_over_their_cap() {
    let limiter = GitRateLimiter::default();
    assert_eq!(limiter.limited_count(), 0);

    for _ in 0..GIT_RATE_LIMIT_ANONYMOUS_MAX {
        limiter.check_anonymous("1.2.3.4");
    }
    assert_eq!(limiter.limited_count(), 1);

    for _ in 0..GIT_RATE_LIMIT_AUTHENTICATED_MAX {
        limiter.check_authenticated("alice");
    }
    assert_eq!(limiter.limited_count(), 2);
}

/// Counts real `get` calls that reach it — what proves a `RequestCache`
/// sitting in front actually shields it, the same role `s3db`'s own
/// `request_cache.rs` tests use one for.
#[derive(Debug)]
struct CountingBackend {
    inner: gitquarry_core::storage::local::LocalBackend,
    gets: AtomicUsize,
}

#[async_trait::async_trait]
impl Backend for CountingBackend {
    async fn get(&self, key: &str) -> anyhow::Result<(Vec<u8>, Option<String>)> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(key).await
    }
    async fn put_cas(&self, key: &str, body: Vec<u8>, expected_etag: Option<&str>) -> anyhow::Result<Cas> {
        self.inner.put_cas(key, body, expected_etag).await
    }
    async fn delete(&self, key: &str) -> anyhow::Result<s3db::DeleteOutcome> {
        self.inner.delete(key).await
    }
    async fn delete_many(&self, keys: &[String]) -> anyhow::Result<s3db::DeleteOutcome> {
        self.inner.delete_many(keys).await
    }
    async fn list_prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, u64)>> {
        self.inner.list_prefix(prefix).await
    }
    async fn put_stream(&self, key: &str, source: &std::path::Path) -> anyhow::Result<()> {
        self.inner.put_stream(key, source).await
    }
    async fn get_stream(&self, key: &str, dest: &std::path::Path) -> anyhow::Result<()> {
        self.inner.get_stream(key, dest).await
    }
    async fn get_range(&self, key: &str, offset: u64, len: u64) -> anyhow::Result<Vec<u8>> {
        self.inner.get_range(key, offset, len).await
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn test_ctx(root: &std::path::Path) -> (AppContext, Arc<CountingBackend>) {
    let inner = gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: gitquarry_core::config::LocalEngine::Files,
    });
    let counting = Arc::new(CountingBackend {
        inner,
        gets: AtomicUsize::new(0),
    });
    let cipher = Arc::new(Cipher::derive("test passphrase", &[4u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
    let ctx = AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend: Arc::clone(&counting) as Arc<dyn Backend>,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        ci: None,
        artifacts: None,
        allow_push_create: false,
        events: Default::default(),
    };
    (ctx, counting)
}

#[tokio::test]
async fn backend_falls_back_to_the_shared_one_outside_any_request_scope() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, counting) = test_ctx(dir.path());
    counting.put_cas("k", b"v".to_vec(), None).await.unwrap();

    ctx.backend().get("k").await.unwrap();
    ctx.backend().get("k").await.unwrap();

    // No `REQUEST_BACKEND` scope active (e.g. ssh://, a background
    // sweep): every call reaches the real backend, same as before this
    // method existed.
    assert_eq!(counting.gets.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn backend_dedupes_repeat_reads_inside_a_request_scope() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, counting) = test_ctx(dir.path());
    counting.put_cas("k", b"v".to_vec(), None).await.unwrap();

    let cached: Arc<dyn Backend> = Arc::new(s3db::RequestCache::new(Arc::clone(&ctx.backend)));
    REQUEST_BACKEND
        .scope(cached, async {
            // Three independent call sites reading the same key within
            // one "request" -- exactly the `login::current_user`
            // pattern `webui::request_cache_middleware` exists to fix.
            ctx.backend().get("k").await.unwrap();
            ctx.backend().get("k").await.unwrap();
            ctx.backend().get("k").await.unwrap();
        })
        .await;

    assert_eq!(counting.gets.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_write_inside_the_scope_still_invalidates_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, counting) = test_ctx(dir.path());
    counting.put_cas("k", b"v1".to_vec(), None).await.unwrap();

    let cached: Arc<dyn Backend> = Arc::new(s3db::RequestCache::new(Arc::clone(&ctx.backend)));
    REQUEST_BACKEND
        .scope(cached, async {
            let (_, etag) = ctx.backend().get("k").await.unwrap();
            ctx.backend()
                .put_cas("k", b"v2".to_vec(), etag.as_deref())
                .await
                .unwrap();
            let (body, _) = ctx.backend().get("k").await.unwrap();
            assert_eq!(body, b"v2");
        })
        .await;

    // Two real reads (before and after the write), not one stale hit.
    assert_eq!(counting.gets.load(Ordering::SeqCst), 2);
}
