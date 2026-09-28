//! `/_users`: the admin-only user/role page, driven through `router()` with
//! a real admin session (`carol`) — and refused to everyone else.

use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Permission, User};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[SALT_BYTE; s3db_crypto::SALT_LEN]).unwrap(),
    );
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

async fn create_user(ctx: &AppContext, username: &str, is_admin: bool) {
    domain::user::create(
        ctx,
        domain::user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
}

async fn send(app: &axum::Router, mut request: Request<Body>) -> Response {
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't add it, and its absence 500s before routing.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    app.clone().oneshot(request).await.unwrap()
}

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::get(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn post_form(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

/// [`post_form`] as `style/app.js` sends a `data-async` submit, which gets
/// back just the fragment its target names.
fn post_async(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let mut request = post_form(uri, cookie, fields);
    request.headers_mut().insert("x-gq-async", "1".parse().unwrap());
    request
}

async fn text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// `name=value` out of the response's `Set-Cookie`, the shape a browser
/// sends back in `Cookie`.
fn cookie_pair(response: &Response, name: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .and_then(|v| v.split(';').next())
        .unwrap_or_else(|| panic!("no Set-Cookie for {name}"))
        .to_string()
}

fn csrf_in(html: &str) -> String {
    let marker = r#"name="csrf" value=""#;
    let start = html.find(marker).unwrap_or_else(|| panic!("no csrf field in {html}")) + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

/// A real password login, returning the session cookie.
async fn log_in(app: &axum::Router, username: &str) -> String {
    let response = send(app, get("/_login", "")).await;
    let anchor = cookie_pair(&response, "login-csrf");
    let csrf = csrf_in(&text(response).await);
    let fields = [("csrf", csrf.as_str()), ("username", username), ("password", PASSWORD)];
    let response = send(app, post_form("/_login", &anchor, &fields)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    cookie_pair(&response, "web-session")
}

/// The page at `uri` for `cookie`'s session, plus the CSRF token its forms
/// carry — read out of the markup the way a browser submits it.
async fn page(app: &axum::Router, uri: &str, cookie: &str) -> (String, String) {
    let response = send(app, get(uri, cookie)).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
    let html = text(response).await;
    let csrf = csrf_in(&html);
    (html, csrf)
}

fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

const SALT_BYTE: u8 = 19;

async fn setup(root: &std::path::Path) -> (Arc<AppContext>, axum::Router, String) {
    let ctx = test_ctx(root);
    create_user(&ctx, "carol", true).await;
    create_user(&ctx, "alice", false).await;
    let ctx = Arc::new(ctx);
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let carol = log_in(&app, "carol").await;
    (ctx, app, carol)
}

async fn user(ctx: &AppContext, username: &str) -> User {
    access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap()
        .0
        .unwrap()
}

async fn users_post(app: &axum::Router, session: &str, fields: &[(&str, &str)]) -> Response {
    let (_html, csrf) = page(app, "/_users", session).await;
    let mut all = vec![("csrf", csrf.as_str())];
    all.extend_from_slice(fields);
    send(app, post_form("/_users", session, &all)).await
}

#[tokio::test]
async fn only_an_admin_reaches_the_page_or_its_actions() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, _carol) = setup(dir.path()).await;

    let response = send(&app, get("/_users", "")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/_login?return_to=/_users");

    let alice = log_in(&app, "alice").await;
    let response = send(&app, get("/_users", &alice)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // Alice's own session and its token, pointed at the admin action.
    let (_html, csrf) = page(&app, "/_account/security", &alice).await;
    let fields = [
        ("csrf", csrf.as_str()),
        ("user_action", "toggle_admin"),
        ("username", "alice"),
    ];
    let response = send(&app, post_form("/_users", &alice, &fields)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!user(&ctx, "alice").await.is_admin);
}

#[tokio::test]
async fn a_users_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("user_action", "toggle_admin"),
        ("username", "alice"),
    ];
    let response = send(&app, post_form("/_users", &carol, &fields)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!user(&ctx, "alice").await.is_admin);
}

#[tokio::test]
async fn users_page_echoes_the_search_query_in_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, carol) = setup(dir.path()).await;
    let (html, _csrf) = page(&app, "/_users?q=ali", &carol).await;
    assert!(html.contains(r#"input name="q" value="ali""#), "{html}");
    assert!(html.contains(r#"id="user-card-alice""#), "{html}");
    assert!(!html.contains(r#"id="user-card-carol""#), "{html}");
}

#[tokio::test]
async fn a_user_card_lists_each_repo_grant_with_a_remove_button() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;
    let public = gitquarry_core::storage::entities::Visibility::Public;
    gitquarry_core::domain::repo::create(
        &ctx,
        "acme/project-a",
        Some("Project A".to_string()),
        None,
        None,
        public,
    )
    .await
    .unwrap()
    .unwrap();
    let en = gitquarry_i18n::Locale::En;
    domain::user::set_repo_perm(&ctx, "alice", "acme/project-a", Some(Permission::Developer), en)
        .await
        .unwrap();
    domain::user::set_repo_perm(&ctx, "alice", "*", Some(Permission::Reporter), en)
        .await
        .unwrap();

    let (html, _csrf) = page(&app, "/_users?q=alice", &carol).await;
    // Label and `full_name`: the row shows the former, its remove form
    // submits the latter.
    assert!(html.contains("<code>acme/Project A</code>"), "{html}");
    assert!(html.contains(r#"name="repo" value="acme/project-a""#), "{html}");
    assert!(html.contains(">developer</span>"), "{html}");
    assert!(html.contains("<code>*</code>"), "{html}");
    assert!(html.contains(">reporter</span>"), "{html}");
    assert!(html.contains(r#"value="remove_repo_perm""#), "{html}");

    let (html, _csrf) = page(&app, "/_users?q=carol", &carol).await;
    assert!(html.contains("(no repos yet"), "{html}");
}

/// Regression: every action whose whole effect stays inside one user's own
/// card targets that card's own `id`; `delete_user` targets `#users-list`
/// instead, since it changes which cards exist. An async submit gets back
/// exactly the fragment its form names. Losing either quietly brings back
/// the browser's "confirm resubmission" dialog on F5 for that action.
#[tokio::test]
async fn card_actions_and_delete_target_the_right_fragment() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, carol) = setup(dir.path()).await;

    let (_html, csrf) = page(&app, "/_users", &carol).await;
    let fields = [
        ("csrf", csrf.as_str()),
        ("user_action", "set_storage_quota"),
        ("username", "alice"),
        ("storage_quota", "5G"),
    ];
    let response = send(&app, post_async("/_users", &carol, &fields)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let card = text(response).await;
    assert!(
        card.starts_with(r#"<div class="card card-pad user-card" id="user-card-alice">"#),
        "{card}"
    );
    assert_eq!(
        card.matches(r##"data-async data-async-target="#user-card-alice""##)
            .count(),
        7,
        "{card}"
    );
    assert_eq!(
        card.matches(r##"data-async data-async-target="#users-list""##).count(),
        1,
        "{card}"
    );

    let fields = [
        ("csrf", csrf.as_str()),
        ("user_action", "delete_user"),
        ("username", "alice"),
    ];
    let response = send(&app, post_async("/_users", &carol, &fields)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let list = text(response).await;
    assert!(list.starts_with(r#"<div id="users-list">"#), "{list}");
    assert!(!list.contains("user-card-alice"), "{list}");
}

#[tokio::test]
async fn a_user_card_offers_totp_disable_only_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;

    let (html, _csrf) = page(&app, "/_users?q=alice", &carol).await;
    assert!(!html.contains(r#"value="disable_totp""#), "{html}");

    let (alice, etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap();
    let mut alice = alice.unwrap();
    alice.totp_enabled = true;
    alice.totp_secret = Some("JBSWY3DPEHPK3PXP".to_string());
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &alice, etag.as_deref())
        .await
        .unwrap();
    let (html, _csrf) = page(&app, "/_users?q=alice", &carol).await;
    assert!(html.contains(r#"value="disable_totp""#), "{html}");

    users_post(&app, &carol, &[("user_action", "disable_totp"), ("username", "alice")]).await;
    assert!(!user(&ctx, "alice").await.totp_enabled);
}

#[tokio::test]
async fn toggle_federation_moderator_flips_the_flag_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, carol) = setup(dir.path()).await;
    let fields = [("user_action", "toggle_federation_moderator"), ("username", "alice")];

    let response = users_post(&app, &carol, &fields).await;
    assert_eq!(response.status(), StatusCode::OK);
    let alice = user(&ctx, "alice").await;
    assert!(alice.is_federation_moderator);
    assert!(!alice.is_admin);

    users_post(&app, &carol, &fields).await;
    assert!(!user(&ctx, "alice").await.is_federation_moderator);
}
