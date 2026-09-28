//! `/_new`: the admin-only form that creates an empty repo under the
//! creator's own name or a group, and its POST.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user::{self, NewUser};
use gitquarry_i18n::Locale;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

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

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let mut request = request;
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

async fn get(app: &Router, uri: &str, cookie: &str) -> (StatusCode, HeaderMap, String) {
    let request = Request::get(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    send(app, request).await
}

async fn post_form(app: &Router, uri: &str, cookie: &str, fields: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let request = Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    send(app, request).await
}

/// `name=value` of the `name` cookie a response sets, the part a browser
/// sends back.
fn set_cookie(headers: &HeaderMap, name: &str) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&format!("{name}=")))
        .and_then(|c| c.split(';').next())
        .unwrap_or_else(|| panic!("no {name} cookie in {headers:?}"))
        .to_string()
}

/// The CSRF token a page's form carries, read the way a browser submits it.
fn csrf_in(html: &str) -> String {
    let marker = r#"name="csrf" value=""#;
    let start = html.find(marker).unwrap_or_else(|| panic!("no csrf field: {html}")) + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

/// A real password login through `/_login`: fetch the form (and the cookie
/// anchoring its CSRF token), submit it, keep the session cookie it sets.
async fn login(app: &Router, username: &str, password: &str) -> String {
    let (_, headers, html) = send(app, Request::get("/_login").body(Body::empty()).unwrap()).await;
    let anchor = set_cookie(&headers, "login-csrf");
    let csrf = csrf_in(&html);
    let (status, headers, body) = post_form(
        app,
        "/_login",
        &anchor,
        &[("username", username), ("password", password), ("csrf", &csrf)],
    )
    .await;
    assert!(status.is_redirection(), "login failed ({status}): {body}");
    set_cookie(&headers, "web-session")
}

async fn create_user(ctx: &AppContext, username: &str, is_admin: bool) {
    user::create(
        ctx,
        NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

struct Site {
    _root: tempfile::TempDir,
    ctx: Arc<AppContext>,
    app: Router,
    /// Admin `alice`'s session cookie.
    alice: String,
}

/// Admin `alice`, non-admin `bob`, and groups `acme` and `widgets`.
async fn site() -> Site {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    create_user(&ctx, "alice", true).await;
    create_user(&ctx, "bob", false).await;
    let alice_uid = gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
        .await
        .unwrap()
        .0
        .unwrap()
        .uid;
    for group in ["acme", "widgets"] {
        gitquarry_core::auth::domain::group::create(&ctx, group, None, alice_uid.clone(), None, Locale::En)
            .await
            .unwrap()
            .unwrap();
    }
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let alice = login(&app, "alice", PASSWORD).await;
    Site {
        _root: root,
        ctx,
        app,
        alice,
    }
}

/// Just the `#new-repo-body` part of the `/_new` page.
fn new_repo_body(html: &str) -> &str {
    let start = html
        .find(r#"<div id="new-repo-body">"#)
        .unwrap_or_else(|| panic!("no #new-repo-body: {html}"));
    &html[start..start + html[start..].find("</form>").unwrap()]
}

#[tokio::test]
async fn new_repo_form_lists_own_username_and_every_group() {
    let s = site().await;
    let (status, _, html) = get(&s.app, "/_new", &s.alice).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(r#"<option value="alice">alice (me)</option>"#), "{html}");
    assert!(html.contains(r#"<option value="acme">acme</option>"#), "{html}");
    assert!(html.contains(r#"<option value="widgets">widgets</option>"#), "{html}");
}

#[tokio::test]
async fn new_repo_form_has_an_optional_template_field() {
    let s = site().await;
    let (_, _, html) = get(&s.app, "/_new", &s.alice).await;
    assert!(new_repo_body(&html).contains(r#"input name="template""#), "{html}");
}

/// Regression: the form is a *plain* POST, not `data-async` — a successful
/// create redirects to the new repo's own page, a real navigation
/// `data-async`'s `fetch()` can't make happen client-side (it would follow
/// the redirect itself and hand the submit handler the destination page's
/// full HTML to graft into `#new-repo-body` as if it were a fragment).
#[tokio::test]
async fn form_is_a_plain_post_not_an_async_fragment_swap() {
    let s = site().await;
    let (_, _, html) = get(&s.app, "/_new", &s.alice).await;
    let form = new_repo_body(&html);
    assert!(form.contains(r#"method="post""#), "{form}");
    assert!(!form.contains("data-async"), "{form}");

    let csrf = csrf_in(&html);
    let (status, headers, body) = post_form(
        &s.app,
        "/_new",
        &s.alice,
        &[("namespace", "acme"), ("name", "web"), ("csrf", &csrf)],
    )
    .await;
    assert!(status.is_redirection(), "{status}: {body}");
    assert_eq!(headers[header::LOCATION], "/acme/web");
    assert!(s.ctx.resolve_repo("acme/web").await.unwrap().is_some());
}

#[tokio::test]
async fn a_taken_name_rerenders_the_form_with_the_flash() {
    let s = site().await;
    let (_, _, html) = get(&s.app, "/_new", &s.alice).await;
    let csrf = csrf_in(&html);
    let fields = [("namespace", "alice"), ("name", "web"), ("csrf", csrf.as_str())];
    let (status, _, _) = post_form(&s.app, "/_new", &s.alice, &fields).await;
    assert!(status.is_redirection());

    let (_, _, html) = post_form(&s.app, "/_new", &s.alice, &fields).await;
    assert!(html.contains(r#"class="alert flash flash-err""#), "{html}");
    assert!(html.contains("a repo with that name already exists."), "{html}");
    assert!(html.contains(r#"<div id="new-repo-body">"#), "{html}");
}

#[tokio::test]
async fn a_namespace_that_is_neither_the_creator_nor_a_group_is_refused() {
    let s = site().await;
    let (_, _, html) = get(&s.app, "/_new", &s.alice).await;
    let csrf = csrf_in(&html);
    let (_, _, html) = post_form(
        &s.app,
        "/_new",
        &s.alice,
        &[("namespace", "nobody"), ("name", "web"), ("csrf", &csrf)],
    )
    .await;
    assert!(html.contains("invalid namespace."), "{html}");
    assert!(s.ctx.resolve_repo("nobody/web").await.unwrap().is_none());
}

#[tokio::test]
async fn a_post_without_the_csrf_token_creates_nothing() {
    let s = site().await;
    let (status, _, _) = post_form(&s.app, "/_new", &s.alice, &[("namespace", "alice"), ("name", "web")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(s.ctx.resolve_repo("alice/web").await.unwrap().is_none());
}

#[tokio::test]
async fn only_an_admin_reaches_the_form() {
    let s = site().await;
    let (status, headers, _) = get(&s.app, "/_new", "").await;
    assert!(status.is_redirection(), "{status}");
    assert!(
        headers[header::LOCATION].to_str().unwrap().starts_with("/_login"),
        "{headers:?}"
    );

    let bob = login(&s.app, "bob", PASSWORD).await;
    let (status, _, _) = get(&s.app, "/_new", &bob).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = post_form(&s.app, "/_new", &bob, &[("namespace", "bob"), ("name", "web")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(s.ctx.resolve_repo("bob/web").await.unwrap().is_none());
}
