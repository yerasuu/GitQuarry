//! The first-boot gate: `/_bootstrap` trades the startup token for a grant
//! cookie, and nothing else opens the admin pages while login is
//! unconfigured.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use tower::ServiceExt;

/// `webui::bootstrap::BOOTSTRAP_COOKIE`, private to the crate.
const BOOTSTRAP_COOKIE: &str = "bootstrap-grant";

fn test_ctx(root: &std::path::Path) -> gitquarry_core::app::AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[9u8; s3db_crypto::SALT_LEN]).unwrap());
    gitquarry_core::app::AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
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

async fn get(ctx: &Arc<gitquarry_core::app::AppContext>, path: &str, cookie: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    gitquarry_webui::webui::router(Arc::clone(ctx))
        .oneshot(request)
        .await
        .unwrap()
}

/// A grant sealed the way `/_bootstrap` seals one, but under `aad` and with
/// an arbitrary deadline — what a forger holding the right cipher output
/// for the wrong purpose, or a stale cookie, would present.
fn sealed_cookie(ctx: &gitquarry_core::app::AppContext, aad: &str, exp: u64) -> String {
    let sealed = ctx
        .cipher
        .seal(
            aad,
            format!(r#"{{"exp":{exp}}}"#).as_bytes(),
            gitquarry_core::crypto::CompressionStrategy::None,
        )
        .unwrap();
    format!(
        "{BOOTSTRAP_COOKIE}={}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sealed)
    )
}

/// One `fmt` layer write, forwarded to whoever holds the receiving end.
struct Capture(Sender<Vec<u8>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf.to_vec()).expect("the capture outlives its subscriber");
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Everything `print_hint` logs, read the way an operator reads it off the
/// console at startup.
async fn startup_log(ctx: &gitquarry_core::app::AppContext) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || Capture(tx.clone()))
        .finish();
    // Thread-scoped, which covers every await below only because
    // `#[tokio::test]`'s runtime is current-thread.
    let guard = tracing::subscriber::set_default(subscriber);
    gitquarry_webui::webui::bootstrap::print_hint(ctx, "127.0.0.1:8080").await;
    drop(guard);
    String::from_utf8(rx.try_iter().flatten().collect()).unwrap()
}

/// The token out of the printed `/_bootstrap?token=…` URL.
async fn printed_token(ctx: &gitquarry_core::app::AppContext) -> String {
    let log = startup_log(ctx).await;
    let marker = "http://127.0.0.1:8080/_bootstrap?token=";
    let start = log
        .find(marker)
        .unwrap_or_else(|| panic!("no bootstrap URL in {log:?}"))
        + marker.len();
    log[start..].split_whitespace().next().unwrap().to_string()
}

/// The `name=value` half of the grant `/_bootstrap` hands out.
async fn exchange_token(ctx: &Arc<gitquarry_core::app::AppContext>) -> String {
    let token = printed_token(ctx).await;
    let response = get(ctx, &format!("/_bootstrap?token={token}"), None).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/_users");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with(&format!("{BOOTSTRAP_COOKIE}=")), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("Max-Age=3600"), "{set_cookie}");
    set_cookie.split(';').next().unwrap().to_string()
}

async fn users_page_status(ctx: &Arc<gitquarry_core::app::AppContext>, cookie: Option<&str>) -> StatusCode {
    get(ctx, "/_users", cookie).await.status()
}

#[tokio::test]
async fn no_cookie_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    assert_eq!(users_page_status(&ctx, None).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_forged_cookie_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    // Plausible-looking base64 that was never sealed by this cipher: the
    // whole point of sealing the grant rather than storing a flag.
    let cookie = format!("{BOOTSTRAP_COOKIE}=eyJleHAiOjk5OTk5OTk5OTl9");
    assert_eq!(users_page_status(&ctx, Some(&cookie)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_cookie_sealed_under_another_name_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    // Right cipher, wrong AAD — a session cookie can't be replayed as a
    // bootstrap grant.
    let cookie = sealed_cookie(&ctx, "web-session", 9_999_999_999);
    assert_eq!(users_page_status(&ctx, Some(&cookie)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_expired_grant_is_no_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    let cookie = sealed_cookie(&ctx, BOOTSTRAP_COOKIE, 1);
    assert_eq!(users_page_status(&ctx, Some(&cookie)).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_printed_token_buys_a_grant_that_opens_the_admin_pages() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    let cookie = exchange_token(&ctx).await;
    assert_eq!(users_page_status(&ctx, Some(&cookie)).await, StatusCode::OK);
}

#[tokio::test]
async fn a_wrong_token_buys_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    for path in ["/_bootstrap", "/_bootstrap?token=", "/_bootstrap?token=00"] {
        let response = get(&ctx, path, None).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "GET {path}");
        assert!(response.headers().get(header::SET_COOKIE).is_none(), "GET {path}");
    }
}

/// Once a password user exists the window is shut: the token no longer
/// trades for a grant, and a grant obtained earlier stops opening anything.
#[tokio::test]
async fn a_configured_login_closes_the_bootstrap_window() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    let token = printed_token(&ctx).await;
    let cookie = exchange_token(&ctx).await;
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: Some("correct horse"),
            is_admin: true,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(startup_log(&ctx).await, "", "print_hint still advertises the token");

    let response = get(&ctx, &format!("/_bootstrap?token={token}"), None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get(header::SET_COOKIE).is_none());

    let response = get(&ctx, "/_users", Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/_login"),
    );
}

#[tokio::test]
async fn the_token_is_long_random_hex() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let token = printed_token(&ctx).await;
    assert_eq!(token.len(), 48, "{token}");
    assert!(token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "{token}");
}
