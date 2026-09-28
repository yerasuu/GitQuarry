//! `/_account/favorites` (the owner's editable starred list) and
//! `/_profile/<user>/favorites` (everyone else's read-only view), driven
//! through `router()` with real sessions.

use gitquarry_core::storage::entities::Visibility;

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

const SALT_BYTE: u8 = 17;

async fn app_with_users(root: &std::path::Path) -> (Arc<AppContext>, axum::Router, String) {
    let ctx = test_ctx(root);
    create_user(&ctx, "alice", false).await;
    create_user(&ctx, "bob", false).await;
    gitquarry_core::domain::repo::create(&ctx, "acme/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let ctx = Arc::new(ctx);
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let session = log_in(&app, "alice").await;
    (ctx, app, session)
}

/// Rewrites `repo`'s stored config in place — the display name, visibility
/// and star count a repo page's own settings would otherwise set.
async fn edit_repo(
    ctx: &AppContext,
    repo: &str,
    edit: impl FnOnce(&mut gitquarry_core::storage::entities::RepoConfig),
) {
    let resolved = ctx.resolve_repo(repo).await.unwrap().unwrap();
    let (mut config, etag) = resolved.remote.get_repo_config().await.unwrap();
    edit(&mut config);
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();
}

async fn star(app: &axum::Router, session: &str, repo: &str, return_to: &str) -> Response {
    let (_html, csrf) = page(app, "/_account/favorites", session).await;
    let fields = [
        ("csrf", csrf.as_str()),
        ("star_action", "toggle_star"),
        ("repo", repo),
        ("return_to", return_to),
    ];
    send(app, post_form("/_account/favorites", session, &fields)).await
}

async fn set_public(app: &axum::Router, session: &str) {
    let (_html, csrf) = page(app, "/_account/favorites", session).await;
    let fields = [
        ("csrf", csrf.as_str()),
        ("favorites_action", "set_visibility"),
        ("favorites_public", "1"),
    ];
    let response = send(app, post_form("/_account/favorites", session, &fields)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn favorites_page_lists_starred_repos_with_an_unstar_button() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;
    assert_eq!(star(&app, &session, "acme/proj", "").await.status(), StatusCode::OK);

    let (html, csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(html.contains(r#"href="/acme/proj""#), "{html}");
    assert!(html.contains(r#"value="toggle_star""#));
    assert!(html.contains(r#"action="/_account/favorites""#));
    // Regression: the row's "remove" form carries `favorites_action=
    // remove_from_list` alongside `star_action=toggle_star` — that's what
    // tells the handler to send back the whole `#favorites-body` (the row
    // just disappeared) instead of the bare star-button fragment a star
    // button elsewhere on the site expects. Losing either attribute here
    // means either a broken async swap or the browser's "confirm
    // resubmission" dialog coming back on F5.
    assert!(html.contains(r#"value="remove_from_list""#), "{html}");
    assert_eq!(
        html.matches(r##"data-async data-async-target="#favorites-body""##)
            .count(),
        2,
        "{html}"
    );

    let fields = [
        ("csrf", csrf.as_str()),
        ("star_action", "toggle_star"),
        ("favorites_action", "remove_from_list"),
        ("repo", "acme/proj"),
    ];
    let fragment = text(send(&app, post_async("/_account/favorites", &session, &fields)).await).await;
    assert!(fragment.starts_with(r#"<div id="favorites-body">"#), "{fragment}");
    assert!(fragment.contains("none yet"), "{fragment}");
}

#[tokio::test]
async fn favorites_page_hints_when_no_repo_is_starred() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;
    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(html.contains("none yet"), "{html}");
}

#[tokio::test]
async fn favorites_visibility_checkbox_reflects_the_current_flag() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;

    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(
        html.contains(r#"<input type="checkbox" name="favorites_public" value="1">"#),
        "{html}"
    );

    set_public(&app, &session).await;
    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(
        html.contains(r#"<input type="checkbox" name="favorites_public" value="1" checked>"#),
        "{html}"
    );
}

#[tokio::test]
async fn favorites_row_shows_the_display_name_and_links_the_slug() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    edit_repo(&ctx, "acme/proj", |c| c.display_name = Some("My Proj!".to_string())).await;
    star(&app, &session, "acme/proj", "").await;

    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(html.contains(r#"href="/acme/proj""#), "{html}");
    assert!(html.contains("acme/My Proj!"), "{html}");
    // The unstar form still carries the slug: that's what the handler
    // resolves, a display name resolves to nothing.
    assert!(html.contains(r#"name="repo" value="acme/proj""#), "{html}");
}

/// The display name is free text, unlike the slug — so it is the field a
/// hostile repo owner controls, on a page other accounts render.
#[tokio::test]
async fn favorites_pages_escape_a_hostile_display_name() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    edit_repo(&ctx, "acme/proj", |c| {
        c.display_name = Some("<script>alert(1)</script>".to_string())
    })
    .await;
    star(&app, &session, "acme/proj", "").await;
    set_public(&app, &session).await;

    let (own, _csrf) = page(&app, "/_account/favorites", &session).await;
    let bob = log_in(&app, "bob").await;
    let public = text(send(&app, get("/_profile/alice/favorites", &bob)).await).await;
    for html in [own, public] {
        assert!(!html.contains("<script>alert"), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{html}");
    }
}

#[tokio::test]
async fn favorites_page_shows_star_count_when_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    star(&app, &session, "acme/proj", "").await;
    set_public(&app, &session).await;

    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(html.contains("1 \u{2605}"), "{html}");
    let public = text(send(&app, get("/_profile/alice/favorites", "")).await).await;
    assert!(public.contains("1 \u{2605}"), "{public}");

    edit_repo(&ctx, "acme/proj", |c| c.star_count = 0).await;
    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(!html.contains('\u{2605}'), "{html}");
}

#[tokio::test]
async fn public_favorites_page_lists_repos_with_no_edit_controls() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;
    star(&app, &session, "acme/proj", "").await;

    // Opt-in: nobody else sees the list until its owner makes it public.
    let response = send(&app, get("/_profile/alice/favorites", "")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = send(&app, get("/_profile/nobody/favorites", "")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    set_public(&app, &session).await;
    let bob = log_in(&app, "bob").await;
    let response = send(&app, get("/_profile/alice/favorites", &bob)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains(r#"href="/acme/proj""#), "{html}");
    assert!(html.contains("<strong>alice</strong>"), "{html}");
    // Read-only: this is someone else's list, only alice's own session can
    // edit it.
    assert!(!html.contains("toggle_star"), "{html}");
    assert!(!html.contains("remove_from_list"), "{html}");
    assert!(!html.contains(r#"action="/_account/favorites""#), "{html}");

    // The owner's own session is sent to the editable page instead.
    let response = send(&app, get("/_profile/alice/favorites", &session)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/_account/favorites");
}

/// A private repo on a public favorites list must not leak to a viewer
/// with no grant on it, even though its owner's own list still shows it.
#[tokio::test]
async fn public_favorites_page_hides_what_the_viewer_cannot_see() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    star(&app, &session, "acme/proj", "").await;
    set_public(&app, &session).await;
    edit_repo(&ctx, "acme/proj", |c| c.visibility = Visibility::Private).await;

    let (own, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(own.contains(r#"href="/acme/proj""#), "{own}");
    let bob = log_in(&app, "bob").await;
    let html = text(send(&app, get("/_profile/alice/favorites", &bob)).await).await;
    assert!(!html.contains("acme/proj"), "{html}");
    assert!(html.contains("none visible"), "{html}");
}

/// Starring would otherwise confirm a private repo exists to anyone who
/// can guess its name.
#[tokio::test]
async fn starring_a_repo_the_viewer_cannot_see_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, session) = app_with_users(dir.path()).await;
    edit_repo(&ctx, "acme/proj", |c| c.visibility = Visibility::Private).await;

    let response = star(&app, &session, "acme/proj", "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(html.contains("none yet"), "{html}");
}

#[tokio::test]
async fn toggling_a_star_only_redirects_to_a_local_return_to() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;

    let response = star(&app, &session, "acme/proj", "/acme/proj").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/acme/proj");

    for hostile in ["//evil.com", "https://evil.com", "/\\evil.com"] {
        let response = star(&app, &session, "acme/proj", hostile).await;
        assert_eq!(response.status(), StatusCode::OK, "{hostile}");
        assert!(response.headers().get(header::LOCATION).is_none(), "{hostile}");
    }
}

#[tokio::test]
async fn a_favorites_post_without_the_session_csrf_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, session) = app_with_users(dir.path()).await;

    let fields = [
        ("csrf", "forged"),
        ("star_action", "toggle_star"),
        ("repo", "acme/proj"),
    ];
    let response = send(&app, post_form("/_account/favorites", &session, &fields)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let (html, _csrf) = page(&app, "/_account/favorites", &session).await;
    assert!(html.contains("none yet"), "{html}");
}
