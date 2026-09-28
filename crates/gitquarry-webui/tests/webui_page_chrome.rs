//! The shared chrome every page is wrapped in: the mobile nav toggle, topic
//! badges, the active-tab marker, and the federation-only entries that must
//! disappear when `[federation]` isn't configured.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery";

fn test_ctx(root: &std::path::Path, federation: bool) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[29u8; s3db_crypto::SALT_LEN]).unwrap());
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
        // Never dialled by the pages below; only its presence changes the
        // chrome they render.
        federation: federation.then(|| gitquarry_core::app::FederationLink {
            base_url: "http://127.0.0.1:1".to_string(),
            internal_token: "unused".to_string(),
            http: reqwest::Client::new(),
        }),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// An admin with a password, so both the admin pages and `/_account/*`
/// open for the session `login` returns.
async fn app_with_admin(root: &std::path::Path, federation: bool) -> axum::Router {
    let ctx = Arc::new(test_ctx(root, federation));
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin: true,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
    gitquarry_webui::webui::router(ctx)
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    app.clone().oneshot(request).await.unwrap()
}

async fn get(app: &axum::Router, path: &str, cookie: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::empty()).unwrap()).await
}

async fn post_form(app: &axum::Router, path: &str, cookie: Option<&str>, fields: &[(&str, &str)]) -> Response {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(app, builder.body(Body::from(body)).unwrap()).await
}

async fn body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The `value` of the first `name="csrf"` field, the way a browser would
/// submit it.
fn csrf_in(html: &str) -> Option<String> {
    let rest = &html[html.find(r#"name="csrf""#)?..];
    let rest = &rest[rest.find(r#"value=""#)? + r#"value=""#.len()..];
    Some(rest[..rest.find('"')?].to_string())
}

/// `name=value` of the `Set-Cookie` for `name`.
fn set_cookie_pair(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap_or_default().to_string())
}

/// `(login-csrf cookie pair, the token the form embeds)`.
async fn login_page(app: &axum::Router) -> (String, String) {
    let response = get(app, "/_login", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let anchor = set_cookie_pair(&response, "login-csrf").expect("no login-csrf cookie");
    let token = csrf_in(&body_text(response).await).expect("no csrf field on /_login");
    (anchor, token)
}

/// The `web-session=...` pair a real password login hands back.
async fn login(app: &axum::Router) -> String {
    let (anchor, token) = login_page(app).await;
    let response = post_form(
        app,
        "/_login",
        Some(&anchor),
        &[
            ("username", "alice"),
            ("password", PASSWORD),
            ("csrf", &token),
            ("return_to", "/"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    set_cookie_pair(&response, "web-session").expect("login set no session cookie")
}

async fn page(app: &axum::Router, path: &str, cookie: Option<&str>) -> String {
    let response = get(app, path, cookie).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {path}");
    body_text(response).await
}

async fn index_with_topics(topics: &[&str]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), false));
    let resolved = ctx.create_repo("acme/web").await.unwrap().unwrap();
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/web".to_string(),
        visibility: Visibility::Public,
        topics: topics.iter().map(|t| t.to_string()).collect(),
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
    gitquarry_core::storage::settings::set_visible_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, true)
        .await;
    let app = gitquarry_webui::webui::router(ctx);
    let html = page(&app, "/", None).await;
    assert!(
        html.contains(r#"href="/acme/web""#),
        "the repo isn't on the index: {html}"
    );
    html
}

#[tokio::test]
async fn every_page_carries_the_mobile_nav_checkbox_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(dir.path(), false)));
    let html = page(&app, "/_license", None).await;
    assert!(html.contains(r#"input type="checkbox" id="nav-toggle" class="nav-toggle-input""#));
    assert!(html.contains(r#"label for="nav-toggle" class="nav-toggle-btn""#));
}

#[tokio::test]
async fn topic_badges_link_to_the_index_search() {
    let html = index_with_topics(&["rust"]).await;
    assert!(html.contains(r#"href="/?q=rust""#), "{html}");
    assert!(html.contains(">rust<"), "{html}");
}

/// Topics are stored as whatever the settings form accepted; the badge is
/// markup built around them, so both the link and the label must escape.
#[tokio::test]
async fn topic_badges_escape_the_topic() {
    let html = index_with_topics(&["<b>x</b>"]).await;
    assert!(!html.contains("<b>x</b>"), "topic rendered as markup");
    assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"), "{html}");
    assert!(html.contains(r#"href="/?q=%3Cb%3Ex%3C/b%3E""#), "{html}");
}

#[tokio::test]
async fn no_topics_render_no_badges() {
    let html = index_with_topics(&[]).await;
    assert!(!html.contains(r#"class="badge accent""#), "{html}");
}

#[tokio::test]
async fn the_tabs_bar_marks_only_the_active_tab_with_aria_current() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(dir.path(), false).await;
    let session = login(&app).await;
    let html = page(&app, "/_users", Some(&session)).await;
    let link_to = |href: &str| {
        let needle = format!(r#"href="{href}""#);
        let at = html.find(&needle).unwrap_or_else(|| panic!("no link to {href}"));
        let open = html[..at].rfind("<a ").unwrap();
        html[open..at + needle.len()].to_string()
    };
    let users = link_to("/_users");
    assert!(users.contains(r#"aria-current="page""#), "{users}");
    let settings = link_to("/_settings/footer");
    assert!(!settings.contains("aria-current"), "{settings}");
}

#[tokio::test]
async fn admin_tabs_drop_peers_without_federation() {
    let with_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(with_dir.path(), true).await;
    let session = login(&app).await;
    let with = page(&app, "/_users", Some(&session)).await;
    assert!(with.contains(r#"href="/_peers""#), "{with}");

    let without_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(without_dir.path(), false).await;
    let session = login(&app).await;
    let without = page(&app, "/_users", Some(&session)).await;
    assert!(!without.contains("/_peers"), "{without}");
    // The rest of the bar is untouched -- only the federation entry goes.
    assert!(without.contains(r#"href="/_users""#), "{without}");
    assert!(without.contains(r#"href="/_audit""#), "{without}");
}

#[tokio::test]
async fn account_sidebar_drops_tokens_without_federation() {
    let with_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(with_dir.path(), true).await;
    let session = login(&app).await;
    let with = page(&app, "/_account/pat", Some(&session)).await;
    assert!(with.contains(r#"href="/_account/tokens""#), "{with}");

    let without_dir = tempfile::tempdir().unwrap();
    let app = app_with_admin(without_dir.path(), false).await;
    let session = login(&app).await;
    let without = page(&app, "/_account/pat", Some(&session)).await;
    assert!(!without.contains("/_account/tokens"), "{without}");
    assert!(without.contains(r#"href="/_account/pat""#), "{without}");
}
