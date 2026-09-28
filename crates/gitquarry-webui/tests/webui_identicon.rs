//! The generated avatar every account without an uploaded one gets: stable
//! per username, distinct across usernames, and decorative.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user::{self, NewUser};
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
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
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

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// A router over a bucket holding `usernames`, none with an uploaded
/// avatar.
async fn app_with_users(root: &Path, usernames: &[&str]) -> Router {
    let ctx = Arc::new(test_ctx(root));
    for username in usernames {
        user::create(
            &ctx,
            NewUser {
                username,
                email: None,
                pubkey: None,
                password_plain: None,
                is_admin: false,
                repos_field: "",
                locale: gitquarry_i18n::Locale::En,
            },
        )
        .await
        .unwrap()
        .unwrap();
    }
    gitquarry_webui::webui::router(ctx)
}

/// The avatar markup heading `username`'s profile page.
async fn profile_avatar(app: &Router, username: &str) -> String {
    let (status, _, html) = get(app, &format!("/_profile/{username}"), None).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let heading = &html[html.find("<h3>").unwrap()..];
    let start = heading
        .find("<svg")
        .unwrap_or_else(|| panic!("no identicon on the profile: {heading}"));
    let end = heading.find("</svg>").unwrap() + "</svg>".len();
    heading[start..end].to_string()
}

#[tokio::test]
async fn same_username_renders_the_same_avatar_on_every_load() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_users(dir.path(), &["alice"]).await;
    assert_eq!(profile_avatar(&app, "alice").await, profile_avatar(&app, "alice").await);

    // Computed from the username alone, so a second instance over another
    // bucket draws the same one.
    let other = tempfile::tempdir().unwrap();
    let other_app = app_with_users(other.path(), &["alice"]).await;
    assert_eq!(
        profile_avatar(&app, "alice").await,
        profile_avatar(&other_app, "alice").await
    );
}

#[tokio::test]
async fn different_usernames_render_different_avatars() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_users(dir.path(), &["alice", "bob"]).await;
    assert_ne!(profile_avatar(&app, "alice").await, profile_avatar(&app, "bob").await);
}

/// Decorative (the username is always written out beside it, so a screen
/// reader announcing the icon too would just repeat it), and carrying no
/// pixel size of its own: `.avatar` sizes it off the surrounding text, so
/// it keeps its proportion to the name under every density and text size.
#[tokio::test]
async fn is_decorative_and_sized_by_the_stylesheet() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_users(dir.path(), &["alice"]).await;
    let svg = profile_avatar(&app, "alice").await;
    assert!(svg.contains(r#"aria-hidden="true""#), "{svg}");
    assert!(svg.contains(r#"class="avatar""#), "{svg}");
    let open_tag = &svg[..svg.find('>').unwrap()];
    assert!(
        !open_tag.contains(" width="),
        "a hardcoded pixel size came back: {open_tag}"
    );
}
