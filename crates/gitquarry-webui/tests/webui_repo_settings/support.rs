//! The instance every repo-settings test drives: a local bucket, real
//! password logins, and requests sent through `webui::router`.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user::{self, NewUser};
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_i18n::Locale;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";
pub const REPO: &str = "alice/proj";

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; s3db_crypto::SALT_LEN]).unwrap());
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

pub struct Page {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Page {
    /// `name=value` of the `Set-Cookie` planting `name`, ready to send back.
    pub fn cookie(&self, name: &str) -> String {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with(&format!("{name}=")))
            .and_then(|c| c.split(';').next())
            .unwrap_or_else(|| panic!("no {name} cookie in {:?}", self.headers))
            .to_string()
    }

    pub fn location(&self) -> &str {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    }

    /// The `csrf` hidden field's value, read out of the markup the way a
    /// browser submitting the form would.
    pub fn csrf(&self) -> String {
        let marker = r#"name="csrf" value=""#;
        let start = self.body.find(marker).expect("page carries no csrf field") + marker.len();
        self.body[start..].split('"').next().unwrap().to_string()
    }
}

pub struct Session {
    pub cookie: String,
    pub csrf: String,
}

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
        let ctx = Arc::new(test_ctx(dir.path()));
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
        let new_user = NewUser {
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

    pub async fn send(&self, method: &str, path: &str, cookie: Option<&str>, form: Option<&[(&str, &str)]>) -> Page {
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

    pub async fn get(&self, path: &str, session: Option<&Session>) -> Page {
        self.send("GET", path, session.map(|s| s.cookie.as_str()), None).await
    }

    /// Submits `fields` plus the session's own CSRF token.
    pub async fn post(&self, path: &str, session: &Session, fields: &[(&str, &str)]) -> Page {
        let mut all = vec![("csrf", session.csrf.as_str())];
        all.extend_from_slice(fields);
        self.send("POST", path, Some(&session.cookie), Some(&all)).await
    }

    /// `POST /{repo}`, the one route every settings sub-form submits to.
    pub async fn save(&self, session: &Session, fields: &[(&str, &str)]) -> Page {
        self.post(&format!("/{REPO}"), session, fields).await
    }

    /// A real password login. The CSRF token comes from an account page
    /// every logged-in user can open, since only admins can open the repo
    /// settings GET but maintainers may still POST to it.
    pub async fn login(&self, username: &str) -> Session {
        let form = self.get("/_login", None).await;
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
        let cookie = done.cookie("web-session");
        let account = self.send("GET", "/_account/appearance", Some(&cookie), None).await;
        assert_eq!(account.status, StatusCode::OK);
        Session {
            csrf: account.csrf(),
            cookie,
        }
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
