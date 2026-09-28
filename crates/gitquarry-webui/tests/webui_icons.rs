//! The inline-SVG icons as the pages actually serve them.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use gitquarry_i18n::Locale;
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

/// How many icon functions `webui::icons` defines.
const ICON_COUNT: usize = 13;

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[26u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
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

async fn git(dir: &Path, args: &[&str]) -> String {
    // Signing is forced off so a developer's global `commit.gpgsign` can't
    // turn every commit here into a pinentry prompt.
    let out = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    form: Option<&[(&str, &str)]>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let body = match form {
        Some(pairs) => {
            builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            Body::from(
                form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(pairs)
                    .finish(),
            )
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (parts.status, parts.headers, String::from_utf8(bytes.to_vec()).unwrap())
}

fn set_cookie_value(headers: &HeaderMap, name: &str) -> String {
    let prefix = format!("{name}=");
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.strip_prefix(&prefix))
        .map(|v| v.split(';').next().unwrap().to_string())
        .unwrap_or_else(|| panic!("no {name} cookie set: {headers:?}"))
}

/// A real password login, CSRF token read off the login form the way a
/// browser submits it; returns the `Cookie` header carrying the session.
async fn login(app: &Router, username: &str) -> String {
    let (_, headers, html) = send(app, "GET", "/_login", None, None).await;
    let anchor = format!("login-csrf={}", set_cookie_value(&headers, "login-csrf"));
    let csrf = html
        .split(r#"name="csrf" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no csrf field: {html}"))
        .to_string();
    let (status, headers, _) = send(
        app,
        "POST",
        "/_login",
        Some(&anchor),
        Some(&[
            ("username", username),
            ("password", PASSWORD),
            ("return_to", "/"),
            ("csrf", &csrf),
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    format!("web-session={}", set_cookie_value(&headers, "web-session"))
}

/// Every page an icon shows up on, seen by a logged-in viewer: a limited
/// (lock badge), archived (archive badge) repo with a README (book),
/// CONTRIBUTING (file), LICENSE (license), a directory (folder), two
/// branches (branch switcher) and a tag.
async fn pages() -> Vec<String> {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Limited)
        .await
        .unwrap()
        .unwrap();
    let (mut config, etag) =
        gitquarry_core::storage::settings::get_repo_config(ctx.backend().as_ref(), repo.uid.as_str(), &ctx.cipher)
            .await
            .unwrap();
    config.archived = true;
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        repo.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::create_dir_all(w.join("src")).unwrap();
    std::fs::write(w.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(w.join("README.md"), "# Hello\n").unwrap();
    std::fs::write(w.join("CONTRIBUTING.md"), "Send patches.\n").unwrap();
    std::fs::write(w.join("LICENSE"), "MIT License\n").unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "initial"]).await;
    git(w, &["tag", "-a", "v1.0", "-m", "first"]).await;
    let sha = git(w, &["rev-parse", "HEAD"]).await;
    let tag_sha = git(w, &["rev-parse", "v1.0"]).await;
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_all_objects(&w.join(".git"), packs.path())
        .await
        .unwrap()
        .unwrap();
    repo.remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let (_, etag) = repo.remote.get_manifest().await.unwrap();
    repo.remote
        .put_manifest(
            &Manifest {
                refs: [
                    ("refs/heads/main".to_string(), sha.clone()),
                    ("refs/heads/feature".to_string(), sha),
                    ("refs/tags/v1.0".to_string(), tag_sha),
                ]
                .into(),
                head: Some("refs/heads/main".to_string()),
            },
            etag.as_deref(),
        )
        .await
        .unwrap();

    user::create(
        &ctx,
        user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin: false,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap();
    let app = gitquarry_webui::webui::router(ctx);
    let cookie = login(&app, "alice").await;

    let mut out = Vec::new();
    for uri in ["/team/proj", "/team/proj?view=tree", "/team/proj?view=refs"] {
        let (status, _, html) = send(&app, "GET", uri, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {html}");
        out.push(html);
    }
    out
}

/// Every distinct `class="icon"` SVG across `pages`, whole.
fn icons(pages: &[String]) -> BTreeSet<String> {
    pages
        .iter()
        .flat_map(|page| page.split("<svg ").skip(1))
        .map(|rest| format!("<svg {}", &rest[..rest.find("</svg>").unwrap() + "</svg>".len()]))
        .filter(|svg| svg.contains(r#"class="icon""#))
        .collect()
}

/// The property tests below only see what these pages serve, so this pins
/// that they reach the whole set rather than passing on a subset.
#[tokio::test]
async fn every_icon_is_served_somewhere() {
    let icons = icons(&pages().await);
    assert_eq!(icons.len(), ICON_COUNT, "{icons:#?}");
}

/// Every icon must stay decorative (never its own accessible name) and
/// sized/colored by the `.icon` CSS class rather than a hardcoded fill.
#[tokio::test]
async fn every_icon_is_decorative_and_uses_the_icon_class() {
    for svg in icons(&pages().await) {
        assert!(svg.contains(r#"aria-hidden="true""#), "{svg}");
        assert!(svg.contains("currentColor"), "{svg}");
    }
}

/// Every shape element must be explicitly closed (`</circle>`, `</path>`,
/// ...) — a bare `;` in maud leaves non-void SVG elements unclosed, which
/// nests every following sibling inside the previous shape and makes
/// browsers paint only the outermost one.
#[tokio::test]
async fn every_icon_closes_each_shape_element() {
    for svg in icons(&pages().await) {
        for shape in ["path", "circle", "rect", "line", "polygon"] {
            assert_eq!(
                svg.matches(&format!("<{shape} ")).count(),
                svg.matches(&format!("</{shape}>")).count(),
                "{shape} not individually closed in {svg}"
            );
        }
    }
}
