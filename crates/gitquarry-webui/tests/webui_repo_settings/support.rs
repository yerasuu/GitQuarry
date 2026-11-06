//! The instance every repo-settings test drives: a local bucket, real
//! password logins, and requests sent through `webui::router`.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_i18n::Locale;
pub use gitquarry_test_support::{Page, Session};
use std::sync::Arc;

pub const REPO: &str = "alice/proj";

pub struct Instance {
    _dir: tempfile::TempDir,
    pub ctx: Arc<AppContext>,
    pub app: axum::Router,
}

impl Instance {
    /// A password user (which switches login enforcement on) named `root`
    /// who is a global admin, `alice` who owns `alice/proj` through its
    /// namespace, and the repo itself.
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        let instance = Self { _dir: dir, ctx, app };
        instance.user("root", true).await;
        instance.user("alice", false).await;
        let owner = gitquarry_core::domain::repo::resolve_owner_ref(&instance.ctx, "alice")
            .await
            .unwrap();
        gitquarry_core::domain::repo::create(&instance.ctx, REPO, None, owner, None, Visibility::Public)
            .await
            .unwrap()
            .unwrap();
        instance
    }

    pub async fn user(&self, username: &str, is_admin: bool) {
        gitquarry_test_support::user(&self.ctx, username, is_admin).await;
    }

    /// Through `domain::repo::set_permission`, the path the permission grid
    /// itself saves through, so the repo's permission index sees the grant.
    pub async fn grant(&self, username: &str, perm: Permission) {
        let uid = self.resolved().await.uid;
        gitquarry_core::domain::repo::set_permission(&self.ctx, &uid, username, Some(perm), Locale::En)
            .await
            .unwrap();
    }

    pub async fn user_with_perm(&self, username: &str, perm: Permission) {
        self.user(username, false).await;
        self.grant(username, perm).await;
    }

    pub async fn get(&self, path: &str, session: Option<&Session>) -> Page {
        Session::get_as(&self.app, path, session.map(|s| s.cookie.as_str())).await
    }

    /// Submits `fields` plus the session's own CSRF token.
    pub async fn post(&self, path: &str, session: &Session, fields: &[(&str, &str)]) -> Page {
        session.post(&self.app, path, fields).await
    }

    /// `POST /{repo}`, the one route every settings sub-form submits to.
    pub async fn save(&self, session: &Session, fields: &[(&str, &str)]) -> Page {
        self.post(&format!("/{REPO}"), session, fields).await
    }

    /// A real password login. The CSRF token comes from an account page
    /// every logged-in user can open, since only admins can open the repo
    /// settings GET but maintainers may still POST to it.
    pub async fn login(&self, username: &str) -> Session {
        Session::login(&self.app, username).await
    }

    pub async fn resolved(&self) -> gitquarry_core::app::ResolvedRepo {
        self.ctx.resolve_repo(REPO).await.unwrap().unwrap()
    }

    pub async fn config(&self) -> gitquarry_core::storage::entities::RepoConfig {
        self.resolved().await.remote.get_repo_config().await.unwrap().0
    }

    /// Branch-picking forms only read ref names out of the manifest, so
    /// placeholder shas stand in for pushed commits.
    pub async fn set_refs(&self, refs: &[&str], head: Option<&str>) {
        let remote = self.resolved().await.remote;
        let (_, etag) = remote.get_manifest().await.unwrap();
        let manifest = gitquarry_core::storage::remote::Manifest {
            refs: refs.iter().map(|r| (r.to_string(), "0".repeat(40))).collect(),
            head: head.map(str::to_string),
        };
        let cas = remote.put_manifest(&manifest, etag.as_deref()).await.unwrap();
        assert!(matches!(cas, gitquarry_core::storage::Cas::Ok));
    }
}

pub fn settings_url(view: &str) -> String {
    format!("/{REPO}?view={view}")
}
