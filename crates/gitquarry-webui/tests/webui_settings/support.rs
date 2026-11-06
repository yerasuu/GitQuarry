//! The instance every server-settings test drives: a local bucket, real
//! password logins, and requests sent through `webui::router`.

use gitquarry_core::app::AppContext;
pub use gitquarry_test_support::{Page, Session};
use std::sync::Arc;

pub struct Instance {
    _dir: tempfile::TempDir,
    pub ctx: Arc<AppContext>,
    pub app: axum::Router,
}

impl Instance {
    /// `root`, a global admin, and `alice`, who isn't one. A password user
    /// existing is also what switches login enforcement on.
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        gitquarry_test_support::user(&ctx, "root", true).await;
        gitquarry_test_support::user(&ctx, "alice", false).await;
        Self { _dir: dir, ctx, app }
    }

    pub async fn get(&self, path: &str, session: Option<&Session>) -> Page {
        Session::get_as(&self.app, path, session.map(|s| s.cookie.as_str())).await
    }

    /// Submits `fields` plus the session's own CSRF token.
    pub async fn post(&self, path: &str, session: &Session, fields: &[(&str, &str)]) -> Page {
        session.post(&self.app, path, fields).await
    }

    /// A real password login. The CSRF token comes from an account page
    /// every logged-in user can open, admin or not.
    pub async fn login(&self, username: &str) -> Session {
        Session::login(&self.app, username).await
    }
}
