//! `/_profile/<name>`, driven through `webui::router`: users and repos are
//! seeded through `gitquarry-core`'s own domain API, viewers log in with a
//! real password.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::user;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_i18n::Locale;
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
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[13u8; s3db_crypto::SALT_LEN]).unwrap());
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

struct Page {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Page {
    fn cookie(&self, name: &str) -> String {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with(&format!("{name}=")))
            .and_then(|c| c.split(';').next())
            .unwrap_or_else(|| panic!("no {name} cookie in {:?}", self.headers))
            .to_string()
    }

    fn csrf(&self) -> String {
        let marker = r#"name="csrf" value=""#;
        let start = self.body.find(marker).expect("page carries no csrf field") + marker.len();
        self.body[start..].split('"').next().unwrap().to_string()
    }

    /// The page's own content, without the topbar/footer every page shares
    /// — the topbar links `/_account/profile` for every logged-in viewer.
    fn main(&self) -> &str {
        let start = self.body.find("<main").expect("no <main>");
        let end = self.body.find("</main>").expect("no </main>");
        &self.body[start..end]
    }
}

struct Instance {
    _dir: tempfile::TempDir,
    ctx: Arc<AppContext>,
    app: axum::Router,
}

impl Instance {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(test_ctx(dir.path()));
        let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
        Self { _dir: dir, ctx, app }
    }

    async fn user(&self, username: &str, is_admin: bool) -> Uid {
        let new_user = user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin,
            repos_field: "",
            locale: Locale::En,
        };
        user::create(&self.ctx, new_user).await.unwrap().unwrap();
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

    async fn send(&self, method: &str, path: &str, cookie: Option<&str>, form: Option<&[(&str, &str)]>) -> Page {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        let body = match form {
            Some(fields) => {
                builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(
                    form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields)
                        .finish(),
                )
            }
            None => Body::empty(),
        };
        let mut request = builder.body(body).unwrap();
        // `run_server` serves with connect info; without it the
        // git-smart-HTTP layer 500s before routing.
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        Page {
            status,
            headers,
            body: String::from_utf8(bytes.to_vec()).unwrap(),
        }
    }

    async fn get(&self, path: &str, session: Option<&str>) -> Page {
        let page = self.send("GET", path, session, None).await;
        assert_eq!(page.status, StatusCode::OK, "GET {path}: {}", page.body);
        page
    }

    /// The `web-session` cookie of a real password login.
    async fn login(&self, username: &str) -> String {
        let form = self.send("GET", "/_login", None, None).await;
        let anchor = form.cookie("login-csrf");
        let csrf = form.csrf();
        let fields = [
            ("csrf", csrf.as_str()),
            ("username", username),
            ("password", PASSWORD),
            ("return_to", "/"),
        ];
        let done = self.send("POST", "/_login", Some(&anchor), Some(&fields)).await;
        assert_eq!(done.status, StatusCode::SEE_OTHER, "login failed: {}", done.body);
        done.cookie("web-session")
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
    assert!(page.main().contains(r#"href="/alice/My-Proj""#), "{}", page.body);
    assert!(page.main().contains("alice/My Proj!"), "{}", page.body);
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
    let secret_row = page.main().split("<tr>").find(|r| r.contains("alice/secret")).unwrap();
    assert!(secret_row.contains("badge"), "{secret_row}");
    let public_row = page
        .main()
        .split("<tr>")
        .find(|r| r.contains("alice/public-one"))
        .unwrap();
    assert!(!public_row.contains("badge"), "{public_row}");

    let root = gq.login("root").await;
    for session in [None, Some(root.as_str())] {
        let page = gq.get("/_profile/alice", session).await;
        assert!(page.main().contains("alice/public-one"), "{}", page.body);
        assert!(!page.main().contains("secret"), "{}", page.body);
    }
}

#[tokio::test]
async fn profile_empty_hint_without_repos() {
    let gq = Instance::new().await;
    gq.user("bob", false).await;
    let page = gq.get("/_profile/bob", None).await;
    assert!(page.main().contains("(no repos visible)"), "{}", page.body);
}

#[tokio::test]
async fn profile_links_to_account_settings_only_for_the_owner() {
    let gq = Instance::new().await;
    gq.user("alice", false).await;
    gq.user("bob", false).await;

    let bob = gq.login("bob").await;
    let someone_elses = gq.get("/_profile/alice", Some(&bob)).await;
    assert!(
        !someone_elses.main().contains("/_account/profile"),
        "{}",
        someone_elses.body
    );

    let alice = gq.login("alice").await;
    let own = gq.get("/_profile/alice", Some(&alice)).await;
    assert!(own.main().contains(r#"href="/_account/profile""#), "{}", own.body);
}

#[tokio::test]
async fn profile_links_to_favorites_only_when_public() {
    let gq = Instance::new().await;
    gq.user("alice", false).await;

    let hidden = gq.get("/_profile/alice", None).await;
    assert!(!hidden.main().contains("/_profile/alice/favorites"), "{}", hidden.body);

    gq.publish_favorites("alice").await;
    let shown = gq.get("/_profile/alice", None).await;
    assert!(
        shown.main().contains(r#"href="/_profile/alice/favorites""#),
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
    let starred_row = page.main().split("<tr>").find(|r| r.contains("alice/starred")).unwrap();
    assert!(starred_row.contains('\u{2605}'), "{starred_row}");
    let unstarred_row = page
        .main()
        .split("<tr>")
        .find(|r| r.contains("alice/unstarred"))
        .unwrap();
    assert!(!unstarred_row.contains('\u{2605}'), "{unstarred_row}");
}
