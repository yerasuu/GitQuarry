//! `/_audit`, driven through `webui::router`: entries are written with
//! `gitquarry_core::storage::audit::record`, the same call every mutating
//! handler makes, and read back as a logged-in viewer.

use axum::http::{StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::audit;
use gitquarry_test_support::{Page, Session};
use std::sync::Arc;

/// The page's own content, without the topbar/footer every page shares
/// — the topbar names the viewer on every page.
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
    /// `root`, a global admin, and `alice`, who isn't one. Creating the first
    /// password user is also what switches login enforcement on, so `/_audit`
    /// stops answering to a bootstrap grant.
    async fn with_users() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        gitquarry_test_support::user(&ctx, "root", true).await;
        gitquarry_test_support::user(&ctx, "alice", false).await;
        Self { _dir: dir, ctx, app }
    }

    async fn audit_page(&self, session: &Session) -> Page {
        let page = session.get(&self.app, "/_audit").await;
        assert_eq!(page.status, StatusCode::OK, "GET /_audit: {}", page.body);
        page
    }
}

#[tokio::test]
async fn an_admin_sees_recorded_entries() {
    let gq = Instance::with_users().await;
    audit::record(
        gq.ctx.backend().as_ref(),
        &gq.ctx.cipher,
        "alice",
        "repo.delete",
        "acme/proj",
        Some("<script>alert(1)</script>".to_string()),
    )
    .await;

    let root = Session::login(&gq.app, "root").await;
    let page = gq.audit_page(&root).await;
    let row = main_of(&page)
        .split("<tr>")
        .find(|r| r.contains("repo.delete"))
        .unwrap();
    assert!(row.contains("alice"), "{row}");
    assert!(row.contains("acme/proj"), "{row}");
    // The detail is whatever the recording handler was handed — often a
    // user-supplied name — so it must come out as text, not markup.
    assert!(row.contains("&lt;script&gt;"), "{row}");
    assert!(!page.body.contains("<script>alert(1)"), "{}", page.body);
}

#[tokio::test]
async fn an_empty_log_shows_the_hint() {
    let gq = Instance::with_users().await;
    let root = Session::login(&gq.app, "root").await;
    let page = gq.audit_page(&root).await;
    assert!(main_of(&page).contains("(empty)"), "{}", page.body);
}

#[tokio::test]
async fn only_an_admin_may_read_the_log() {
    let gq = Instance::with_users().await;
    audit::record(
        gq.ctx.backend().as_ref(),
        &gq.ctx.cipher,
        "root",
        "repo.delete",
        "acme/proj",
        None,
    )
    .await;

    let anonymous = Session::get_as(&gq.app, "/_audit", None).await;
    assert_eq!(anonymous.status, StatusCode::SEE_OTHER);
    assert_eq!(anonymous.headers[header::LOCATION], "/_login?return_to=/_audit");

    let alice = Session::login(&gq.app, "alice").await;
    let page = alice.get(&gq.app, "/_audit").await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert!(!page.body.contains("repo.delete"), "{}", page.body);
}
