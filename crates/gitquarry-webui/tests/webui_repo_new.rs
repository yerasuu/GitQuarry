//! `/_new`: the admin-only form that creates an empty repo under the
//! creator's own name or a group, and its POST.

use axum::Router;
use axum::http::{StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Page, Session, ctx, post_form, user};
use std::sync::Arc;

async fn get(app: &Router, uri: &str, cookie: &str) -> Page {
    Session::get_as(app, uri, Some(cookie)).await
}

async fn login(app: &Router, username: &str) -> String {
    Session::login(app, username).await.cookie
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
    let ctx = Arc::new(ctx(root.path()));
    user(&ctx, "alice", true).await;
    user(&ctx, "bob", false).await;
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
    let alice = login(&app, "alice").await;
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
    let Page { status, body: html, .. } = get(&s.app, "/_new", &s.alice).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(r#"<option value="alice">alice (me)</option>"#), "{html}");
    assert!(html.contains(r#"<option value="acme">acme</option>"#), "{html}");
    assert!(html.contains(r#"<option value="widgets">widgets</option>"#), "{html}");
}

#[tokio::test]
async fn new_repo_form_has_an_optional_template_field() {
    let s = site().await;
    let Page { body: html, .. } = get(&s.app, "/_new", &s.alice).await;
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
    let page = get(&s.app, "/_new", &s.alice).await;
    let form = new_repo_body(&page.body);
    assert!(form.contains(r#"method="post""#), "{form}");
    assert!(!form.contains("data-async"), "{form}");

    let csrf = page.csrf();
    let Page { status, headers, body } = post_form(
        &s.app,
        "/_new",
        Some(&s.alice),
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
    let form = get(&s.app, "/_new", &s.alice).await;
    let csrf = form.csrf();
    let fields = [("namespace", "alice"), ("name", "web"), ("csrf", csrf.as_str())];
    let Page { status, .. } = post_form(&s.app, "/_new", Some(&s.alice), &fields).await;
    assert!(status.is_redirection());

    let Page { body: html, .. } = post_form(&s.app, "/_new", Some(&s.alice), &fields).await;
    assert!(html.contains(r#"class="alert flash flash-err""#), "{html}");
    assert!(html.contains("a repo with that name already exists."), "{html}");
    assert!(html.contains(r#"<div id="new-repo-body">"#), "{html}");
}

#[tokio::test]
async fn a_namespace_that_is_neither_the_creator_nor_a_group_is_refused() {
    let s = site().await;
    let form = get(&s.app, "/_new", &s.alice).await;
    let csrf = form.csrf();
    let Page { body: html, .. } = post_form(
        &s.app,
        "/_new",
        Some(&s.alice),
        &[("namespace", "nobody"), ("name", "web"), ("csrf", &csrf)],
    )
    .await;
    assert!(html.contains("invalid namespace."), "{html}");
    assert!(s.ctx.resolve_repo("nobody/web").await.unwrap().is_none());
}

#[tokio::test]
async fn a_post_without_the_csrf_token_creates_nothing() {
    let s = site().await;
    let Page { status, .. } = post_form(
        &s.app,
        "/_new",
        Some(&s.alice),
        &[("namespace", "alice"), ("name", "web")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(s.ctx.resolve_repo("alice/web").await.unwrap().is_none());
}

#[tokio::test]
async fn only_an_admin_reaches_the_form() {
    let s = site().await;
    let Page { status, headers, .. } = get(&s.app, "/_new", "").await;
    assert!(status.is_redirection(), "{status}");
    assert!(
        headers[header::LOCATION].to_str().unwrap().starts_with("/_login"),
        "{headers:?}"
    );

    let bob = login(&s.app, "bob").await;
    let Page { status, .. } = get(&s.app, "/_new", &bob).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let Page { status, .. } = post_form(&s.app, "/_new", Some(&bob), &[("namespace", "bob"), ("name", "web")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(s.ctx.resolve_repo("bob/web").await.unwrap().is_none());
}
