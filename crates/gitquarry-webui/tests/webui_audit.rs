//! `/_audit`, driven through `webui::router`: entries are written with
//! `gitquarry_core::storage::audit::record`, the same call every mutating
//! handler makes, and read back as a logged-in viewer.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user;
use gitquarry_core::storage::audit;
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
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[15u8; s3db_crypto::SALT_LEN]).unwrap());
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
    /// — the topbar names the viewer on every page.
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

    /// Creating the first password user is also what switches login
    /// enforcement on, so `/_audit` stops answering to a bootstrap grant.
    async fn user(&self, username: &str, is_admin: bool) {
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
    }

    /// `root`, a global admin, and `alice`, who isn't one.
    async fn with_users() -> Self {
        let gq = Self::new().await;
        gq.user("root", true).await;
        gq.user("alice", false).await;
        gq
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

    let root = gq.login("root").await;
    let page = gq.get("/_audit", Some(&root)).await;
    let row = page.main().split("<tr>").find(|r| r.contains("repo.delete")).unwrap();
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
    let root = gq.login("root").await;
    let page = gq.get("/_audit", Some(&root)).await;
    assert!(page.main().contains("(empty)"), "{}", page.body);
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

    let anonymous = gq.send("GET", "/_audit", None, None).await;
    assert_eq!(anonymous.status, StatusCode::SEE_OTHER);
    assert_eq!(anonymous.headers[header::LOCATION], "/_login?return_to=/_audit");

    let alice = gq.login("alice").await;
    let page = gq.send("GET", "/_audit", Some(&alice), None).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert!(!page.body.contains("repo.delete"), "{}", page.body);
}
