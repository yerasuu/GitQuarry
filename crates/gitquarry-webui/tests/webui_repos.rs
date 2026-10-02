//! `/_repos`, driven through `webui::router`: users, groups and repos are
//! seeded through `gitquarry-core`'s own domain API, viewers log in with a
//! real password.

use axum::http::{StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::{group, user};
use gitquarry_core::auth::entities::{Group, Permission};
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Page, Session};
use std::sync::Arc;

/// The page's own content, without the topbar/footer every page shares
/// — the topbar names the viewer and links their account on every page.
fn content(page: &Page) -> &str {
    let start = page.body.find("<main").expect("no <main>");
    let end = page.body.find("</main>").expect("no </main>");
    &page.body[start..end]
}

struct Instance {
    _dir: tempfile::TempDir,
    ctx: Arc<AppContext>,
    app: axum::Router,
}

impl Instance {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        Self { _dir: dir, ctx, app }
    }

    async fn user(&self, username: &str) -> Uid {
        gitquarry_test_support::user(&self.ctx, username, false).await;
        access::get_user(self.ctx.backend().as_ref(), &self.ctx.cipher, username)
            .await
            .unwrap()
            .0
            .unwrap()
            .uid
    }

    async fn group(&self, segment: &str, creator: &Uid, parent: Option<&Group>) -> Group {
        group::create(&self.ctx, segment, None, creator.clone(), parent, Locale::En)
            .await
            .unwrap()
            .unwrap();
        let name = parent.map_or(segment.to_string(), |p| format!("{}/{segment}", p.name));
        access::get_group(self.ctx.backend().as_ref(), &self.ctx.cipher, &name)
            .await
            .unwrap()
            .0
            .unwrap()
    }

    async fn repo(&self, full_name: &str, owner: OwnerRef, visibility: Visibility) {
        gitquarry_core::domain::repo::create(&self.ctx, full_name, None, Some(owner), None, visibility)
            .await
            .unwrap()
            .unwrap();
    }

    async fn get(&self, path: &str, session: Option<&str>) -> Page {
        let page = Session::get_as(&self.app, path, session).await;
        assert_eq!(page.status, StatusCode::OK, "GET {path}: {}", page.body);
        page
    }

    /// The `web-session` cookie of a real password login.
    async fn login(&self, username: &str) -> String {
        Session::login(&self.app, username).await.cookie
    }
}

/// Every tab is "repos *you* reach": an anonymous visitor is sent to log in
/// and brought back here.
#[tokio::test]
async fn an_anonymous_visitor_is_sent_to_log_in() {
    let gq = Instance::new().await;
    let page = Session::get_as(&gq.app, "/_repos", None).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert_eq!(page.headers[header::LOCATION], "/_login?return_to=/_repos");
}

#[tokio::test]
async fn yours_tab_lists_personal_repos_including_private_ones() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    gq.repo("alice/web", OwnerRef::User(alice.clone()), Visibility::Public)
        .await;
    gq.repo("alice/secret", OwnerRef::User(alice), Visibility::Private)
        .await;

    let session = gq.login("alice").await;
    let page = gq.get("/_repos", Some(&session)).await;
    assert!(content(&page).contains(r#"href="/alice/web""#), "{}", page.body);
    let secret_row = content(&page)
        .split("<tr>")
        .find(|r| r.contains("alice/secret"))
        .unwrap();
    assert!(secret_row.contains("private"), "{secret_row}");
}

#[tokio::test]
async fn yours_tab_says_so_when_there_is_nothing() {
    let gq = Instance::new().await;
    gq.user("bob").await;
    let session = gq.login("bob").await;
    let page = gq.get("/_repos", Some(&session)).await;
    assert!(content(&page).contains("don't own any repo yet"), "{}", page.body);
}

/// The split is the point of this page: each group gets its own section
/// headed by a link to that group, and a subgroup is its own section under
/// its full path rather than folded into the parent's — separate namespace,
/// separate repos.
#[tokio::test]
async fn groups_tab_keeps_one_section_per_group_linked_to_its_page() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    let acme = gq.group("acme", &alice, None).await;
    let team = gq.group("team", &alice, Some(&acme)).await;
    gq.repo("acme/web", OwnerRef::Group(acme.uid), Visibility::Private)
        .await;
    gq.repo("acme/team/api", OwnerRef::Group(team.uid), Visibility::Public)
        .await;

    let session = gq.login("alice").await;
    let page = gq.get("/_repos?tab=groups", Some(&session)).await;
    for href in [
        r#"href="/_group/acme""#,
        r#"href="/_group/acme/team""#,
        r#"href="/acme/web""#,
        r#"href="/acme/team/api""#,
    ] {
        assert!(content(&page).contains(href), "{href} missing: {}", page.body);
    }
}

#[tokio::test]
async fn groups_tab_says_so_when_the_viewer_has_no_group() {
    let gq = Instance::new().await;
    gq.user("bob").await;
    let session = gq.login("bob").await;
    let page = gq.get("/_repos?tab=groups", Some(&session)).await;
    assert!(content(&page).contains("don't belong to any group"), "{}", page.body);
}

#[tokio::test]
async fn shared_tab_lists_individually_granted_repos() {
    let gq = Instance::new().await;
    gq.user("alice").await;
    let bob = gq.user("bob").await;
    gq.repo("bob/tool", OwnerRef::User(bob), Visibility::Private).await;
    user::set_repo_perm(&gq.ctx, "alice", "bob/tool", Some(Permission::Reporter), Locale::En)
        .await
        .unwrap();

    let alice = gq.login("alice").await;
    let page = gq.get("/_repos?tab=shared", Some(&alice)).await;
    assert!(content(&page).contains(r#"href="/bob/tool""#), "{}", page.body);

    let bob = gq.login("bob").await;
    let empty = gq.get("/_repos?tab=shared", Some(&bob)).await;
    assert!(content(&empty).contains("nobody granted you access"), "{}", empty.body);
}
