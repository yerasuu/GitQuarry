//! Passkeys through `webui::router`: the public URL they hang off, the
//! registration ceremony on `/_account/security`, passkey-only sign-in from
//! `/_login` and the passkey as a second factor from `/_login_totp`, every
//! response produced by the shared software authenticator.

use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage;
use gitquarry_test_support::{Session, ctx, user};
use std::sync::Arc;

const PUBLIC_URL: &str = "https://git.example.com";

struct Instance {
    _dir: tempfile::TempDir,
    ctx: Arc<AppContext>,
    app: axum::Router,
}

impl Instance {
    /// `root` (an admin) and `alice`, both password users.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(ctx(dir.path()));
        user(&ctx, "root", true).await;
        user(&ctx, "alice", false).await;
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        Instance { _dir: dir, ctx, app }
    }

    async fn stored_public_url(&self) -> Option<String> {
        storage::settings::get_settings(self.ctx.backend().as_ref(), &self.ctx.cipher)
            .await
            .unwrap()
            .0
            .public_url
    }

    async fn save_public_url(&self, admin: &Session, url: &str) -> gitquarry_test_support::Page {
        admin
            .post(&self.app, "/_settings/network", &[("public_url", url)])
            .await
    }
}

#[tokio::test]
async fn the_public_url_setting_accepts_https_domains_and_localhost_only() {
    let gq = Instance::new().await;
    let root = Session::login(&gq.app, "root").await;

    let page = root.get(&gq.app, "/_settings/network").await;
    assert!(page.body.contains(r#"name="public_url""#), "{}", page.body);

    for bad in [
        "http://git.example.com",
        "https://192.168.1.10",
        "https://[::1]",
        "ftp://git.example.com",
        "not a url",
    ] {
        let page = gq.save_public_url(&root, bad).await;
        assert!(page.body.contains("flash-err"), "{bad} accepted: {}", page.body);
        assert_eq!(gq.stored_public_url().await, None, "{bad} was stored");
    }

    for good in ["http://localhost:8080", PUBLIC_URL] {
        let page = gq.save_public_url(&root, good).await;
        assert!(page.body.contains("flash-ok"), "{good} refused: {}", page.body);
        assert_eq!(gq.stored_public_url().await.as_deref(), Some(good));
    }

    // Blank switches passkeys back off.
    gq.save_public_url(&root, "  ").await;
    assert_eq!(gq.stored_public_url().await, None);

    // Admin-only, like the rest of the page.
    let alice = Session::login(&gq.app, "alice").await;
    let page = gq.save_public_url(&alice, PUBLIC_URL).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert_eq!(gq.stored_public_url().await, None);
}
