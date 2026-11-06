//! `/_group/<path>`, driven through `webui::router`: groups, members and
//! repos are seeded through `gitquarry-core`'s own domain API, viewers log
//! in with a real password.

use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::group;
use gitquarry_core::auth::entities::Group;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Page, Session};
use std::sync::Arc;

/// The page's own content, without the topbar/footer every page shares
/// — the topbar links `/_groups` and names the viewer on every page.
fn main_of(page: &Page) -> &str {
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

    async fn add_member(&self, group_name: &str, username: &str, permission: &str) {
        let (mut group, etag) = access::get_group(self.ctx.backend().as_ref(), &self.ctx.cipher, group_name)
            .await
            .unwrap();
        let group = group.as_mut().unwrap();
        group::set_member(&self.ctx, group, username, permission, Locale::En)
            .await
            .unwrap();
        access::put_group(self.ctx.backend().as_ref(), &self.ctx.cipher, group, etag.as_deref())
            .await
            .unwrap();
    }

    async fn repo(&self, full_name: &str, owner: &Group, visibility: Visibility) {
        let owner = Some(OwnerRef::Group(owner.uid.clone()));
        gitquarry_core::domain::repo::create(&self.ctx, full_name, None, owner, None, visibility)
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

#[tokio::test]
async fn renders_the_groups_repos_and_subgroups() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    let acme = gq.group("acme", &alice, None).await;
    gq.group("team", &alice, Some(&acme)).await;
    gq.repo("acme/web", &acme, Visibility::Public).await;

    let page = gq.get("/_group/acme", None).await;
    assert!(main_of(&page).contains(r#"href="/acme/web""#), "{}", page.body);
    assert!(main_of(&page).contains(r#"href="/_group/acme/team""#), "{}", page.body);
}

/// Who belongs to a group, and its private repos, are only shown to the
/// group itself — anonymous visitors and logged-in outsiders see neither.
#[tokio::test]
async fn hides_members_and_private_repos_from_a_non_member() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    gq.user("bob").await;
    gq.user("carol").await;
    let acme = gq.group("acme", &alice, None).await;
    gq.add_member("acme", "bob", "developer").await;
    gq.repo("acme/secret", &acme, Visibility::Private).await;

    let carol = gq.login("carol").await;
    for session in [None, Some(carol.as_str())] {
        let page = gq.get("/_group/acme", session).await;
        assert!(!main_of(&page).contains("alice"), "{}", page.body);
        assert!(!main_of(&page).contains("acme/secret"), "{}", page.body);
        assert!(main_of(&page).contains("only the group's own members see who belongs to it"));
    }

    let bob = gq.login("bob").await;
    let page = gq.get("/_group/acme", Some(&bob)).await;
    assert!(main_of(&page).contains(r#"href="/_profile/alice""#), "{}", page.body);
    assert!(main_of(&page).contains(r#"href="/acme/secret""#), "{}", page.body);
}

/// A subgroup's page has to show the members it inherited — that inheritance
/// is what subgroups are for — but mark them, or the page reads as though
/// this group granted access it never did.
#[tokio::test]
async fn marks_a_member_whose_role_comes_from_a_parent() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    let bob = gq.user("bob").await;
    let acme = gq.group("acme", &alice, None).await;
    gq.group("team", &bob, Some(&acme)).await;

    let session = gq.login("bob").await;
    let page = gq.get("/_group/acme/team", Some(&session)).await;
    assert!(main_of(&page).contains(r#"href="/_profile/alice""#), "{}", page.body);
    assert!(main_of(&page).contains(r#"href="/_profile/bob""#), "{}", page.body);
    assert_eq!(main_of(&page).matches("(inherited)").count(), 1, "{}", page.body);
}

/// The breadcrumb is the only way back up from a subgroup page.
#[tokio::test]
async fn links_every_ancestor_in_the_breadcrumb() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    let acme = gq.group("acme", &alice, None).await;
    gq.group("team", &alice, Some(&acme)).await;

    let page = gq.get("/_group/acme/team", None).await;
    assert!(main_of(&page).contains(r#"href="/_group/acme""#), "{}", page.body);
    // The page's own group is the last crumb, not a link to itself.
    assert!(!main_of(&page).contains(r#"href="/_group/acme/team""#), "{}", page.body);
}

#[tokio::test]
async fn offers_a_manage_link_only_to_someone_who_may_manage() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    gq.user("bob").await;
    gq.group("acme", &alice, None).await;
    gq.add_member("acme", "bob", "developer").await;

    let bob = gq.login("bob").await;
    let plain = gq.get("/_group/acme", Some(&bob)).await;
    assert!(!main_of(&plain).contains("manage this group"), "{}", plain.body);

    let alice = gq.login("alice").await;
    let owner = gq.get("/_group/acme", Some(&alice)).await;
    assert!(main_of(&owner).contains(r#"href="/_groups""#), "{}", owner.body);
    assert!(main_of(&owner).contains("manage this group"), "{}", owner.body);
}
