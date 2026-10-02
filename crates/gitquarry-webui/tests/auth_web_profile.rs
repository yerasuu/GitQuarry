//! `/_profile/<name>`, driven through `webui::router`: users and repos are
//! seeded through `gitquarry-core`'s own domain API, viewers log in with a
//! real password.

use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::{Page, Session, ctx};
use std::sync::Arc;

/// The page's own content, without the topbar/footer every page shares
/// — the topbar links `/_account/profile` for every logged-in viewer.
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
        let ctx = Arc::new(ctx(dir.path()));
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        Self { _dir: dir, ctx, app }
    }

    async fn user(&self, username: &str, is_admin: bool) -> Uid {
        gitquarry_test_support::user(&self.ctx, username, is_admin).await;
        access::get_user(self.ctx.backend().as_ref(), &self.ctx.cipher, username)
            .await
            .unwrap()
            .0
            .unwrap()
            .uid
    }

    async fn repo(&self, full_name: &str, owner: &Uid, display_name: Option<&str>, visibility: Visibility) -> Uid {
        let owner = Some(OwnerRef::User(owner.clone()));
        gitquarry_core::domain::repo::create(
            &self.ctx,
            full_name,
            display_name.map(str::to_string),
            owner,
            None,
            visibility,
        )
        .await
        .unwrap()
        .unwrap()
        .uid
    }

    /// What `/_account/favorites`' visibility toggle stores.
    async fn publish_favorites(&self, username: &str) {
        let (user, etag) = access::get_user(self.ctx.backend().as_ref(), &self.ctx.cipher, username)
            .await
            .unwrap();
        let mut user = user.unwrap();
        user.favorites_public = true;
        access::put_user(self.ctx.backend().as_ref(), &self.ctx.cipher, &user, etag.as_deref())
            .await
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
async fn profile_row_shows_the_display_name_and_links_the_slug() {
    let gq = Instance::new().await;
    let alice = gq.user("alice", false).await;
    // `My-Proj` is the slug `domain::repo::backfill_slug` derives from the
    // display name; any other `full_name` gets rewritten to it on listing.
    gq.repo("alice/My-Proj", &alice, Some("My Proj!"), Visibility::Public)
        .await;

    let page = gq.get("/_profile/alice", None).await;
    assert!(content(&page).contains(r#"href="/alice/My-Proj""#), "{}", page.body);
    assert!(content(&page).contains("alice/My Proj!"), "{}", page.body);
}

/// Only the owner's own session lists their private repos — not an admin,
/// not an anonymous visitor.
#[tokio::test]
async fn profile_marks_private_repos_and_shows_them_only_to_their_owner() {
    let gq = Instance::new().await;
    let alice = gq.user("alice", false).await;
    gq.user("root", true).await;
    gq.repo("alice/public-one", &alice, None, Visibility::Public).await;
    gq.repo("alice/secret", &alice, None, Visibility::Private).await;

    let own = gq.login("alice").await;
    let page = gq.get("/_profile/alice", Some(&own)).await;
    let secret_row = content(&page)
        .split("<tr>")
        .find(|r| r.contains("alice/secret"))
        .unwrap();
    assert!(secret_row.contains("badge"), "{secret_row}");
    let public_row = content(&page)
        .split("<tr>")
        .find(|r| r.contains("alice/public-one"))
        .unwrap();
    assert!(!public_row.contains("badge"), "{public_row}");

    let root = gq.login("root").await;
    for session in [None, Some(root.as_str())] {
        let page = gq.get("/_profile/alice", session).await;
        assert!(content(&page).contains("alice/public-one"), "{}", page.body);
        assert!(!content(&page).contains("secret"), "{}", page.body);
    }
}

#[tokio::test]
async fn profile_empty_hint_without_repos() {
    let gq = Instance::new().await;
    gq.user("bob", false).await;
    let page = gq.get("/_profile/bob", None).await;
    assert!(content(&page).contains("(no repos visible)"), "{}", page.body);
}

#[tokio::test]
async fn profile_links_to_account_settings_only_for_the_owner() {
    let gq = Instance::new().await;
    gq.user("alice", false).await;
    gq.user("bob", false).await;

    let bob = gq.login("bob").await;
    let someone_elses = gq.get("/_profile/alice", Some(&bob)).await;
    assert!(
        !content(&someone_elses).contains("/_account/profile"),
        "{}",
        someone_elses.body
    );

    let alice = gq.login("alice").await;
    let own = gq.get("/_profile/alice", Some(&alice)).await;
    assert!(content(&own).contains(r#"href="/_account/profile""#), "{}", own.body);
}

#[tokio::test]
async fn profile_links_to_favorites_only_when_public() {
    let gq = Instance::new().await;
    gq.user("alice", false).await;

    let hidden = gq.get("/_profile/alice", None).await;
    assert!(
        !content(&hidden).contains("/_profile/alice/favorites"),
        "{}",
        hidden.body
    );

    gq.publish_favorites("alice").await;
    let shown = gq.get("/_profile/alice", None).await;
    assert!(
        content(&shown).contains(r#"href="/_profile/alice/favorites""#),
        "{}",
        shown.body
    );
}

#[tokio::test]
async fn profile_shows_star_count_only_for_starred_repos() {
    let gq = Instance::new().await;
    let alice = gq.user("alice", false).await;
    let starred = gq.repo("alice/starred", &alice, None, Visibility::Public).await;
    gq.repo("alice/unstarred", &alice, None, Visibility::Public).await;
    gitquarry_core::domain::repo::toggle_star(&gq.ctx, "alice", &starred)
        .await
        .unwrap()
        .unwrap();

    let page = gq.get("/_profile/alice", None).await;
    let starred_row = content(&page)
        .split("<tr>")
        .find(|r| r.contains("alice/starred"))
        .unwrap();
    assert!(starred_row.contains('\u{2605}'), "{starred_row}");
    let unstarred_row = content(&page)
        .split("<tr>")
        .find(|r| r.contains("alice/unstarred"))
        .unwrap();
    assert!(!unstarred_row.contains('\u{2605}'), "{unstarred_row}");
}
