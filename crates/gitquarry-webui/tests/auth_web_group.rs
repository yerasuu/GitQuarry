//! `/_group/<path>`, driven through `webui::router`: groups, members and
//! repos are seeded through `gitquarry-core`'s own domain API, viewers log
//! in with a real password.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain::{group, user};
use gitquarry_core::auth::entities::Group;
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
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[12u8; s3db_crypto::SALT_LEN]).unwrap());
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
    /// — the topbar links `/_groups` and names the viewer on every page.
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

    async fn user(&self, username: &str) -> Uid {
        let new_user = user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin: false,
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
async fn renders_the_groups_repos_and_subgroups() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    let acme = gq.group("acme", &alice, None).await;
    gq.group("team", &alice, Some(&acme)).await;
    gq.repo("acme/web", &acme, Visibility::Public).await;

    let page = gq.get("/_group/acme", None).await;
    assert!(page.main().contains(r#"href="/acme/web""#), "{}", page.body);
    assert!(page.main().contains(r#"href="/_group/acme/team""#), "{}", page.body);
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
        assert!(!page.main().contains("alice"), "{}", page.body);
        assert!(!page.main().contains("acme/secret"), "{}", page.body);
        assert!(
            page.main()
                .contains("only the group's own members see who belongs to it")
        );
    }

    let bob = gq.login("bob").await;
    let page = gq.get("/_group/acme", Some(&bob)).await;
    assert!(page.main().contains(r#"href="/_profile/alice""#), "{}", page.body);
    assert!(page.main().contains(r#"href="/acme/secret""#), "{}", page.body);
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
    assert!(page.main().contains(r#"href="/_profile/alice""#), "{}", page.body);
    assert!(page.main().contains(r#"href="/_profile/bob""#), "{}", page.body);
    assert_eq!(page.main().matches("(inherited)").count(), 1, "{}", page.body);
}

/// The breadcrumb is the only way back up from a subgroup page.
#[tokio::test]
async fn links_every_ancestor_in_the_breadcrumb() {
    let gq = Instance::new().await;
    let alice = gq.user("alice").await;
    let acme = gq.group("acme", &alice, None).await;
    gq.group("team", &alice, Some(&acme)).await;

    let page = gq.get("/_group/acme/team", None).await;
    assert!(page.main().contains(r#"href="/_group/acme""#), "{}", page.body);
    // The page's own group is the last crumb, not a link to itself.
    assert!(!page.main().contains(r#"href="/_group/acme/team""#), "{}", page.body);
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
    assert!(!plain.main().contains("manage this group"), "{}", plain.body);

    let alice = gq.login("alice").await;
    let owner = gq.get("/_group/acme", Some(&alice)).await;
    assert!(owner.main().contains(r#"href="/_groups""#), "{}", owner.body);
    assert!(owner.main().contains("manage this group"), "{}", owner.body);
}
