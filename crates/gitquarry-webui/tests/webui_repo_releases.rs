//! The `releases` tab and a release's own page: who sees drafts and the
//! write forms, what the tag field suggests, and how assets are labelled.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::domain::user::{self, NewUser};
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::{AssetMeta, Manifest, ReleaseMeta};
use gitquarry_i18n::Locale;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

fn test_ctx(root: &Path) -> AppContext {
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
        .env("GIT_AUTHOR_DATE", "2024-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2024-01-01T00:00:00Z")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// What a `git push` of every ref in `work` leaves in the bucket: its
/// objects as one uploaded pack, its refs in the manifest.
async fn push_all(ctx: &AppContext, name: &str, work: &Path) {
    let resolved = ctx.resolve_repo(name).await.unwrap().unwrap();
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_all_objects(&work.join(".git"), packs.path())
        .await
        .unwrap()
        .unwrap();
    resolved
        .remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let refs = git(work, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .await
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(name, sha)| (name.to_string(), sha.to_string()))
        .collect();
    let (_, etag) = resolved.remote.get_manifest().await.unwrap();
    resolved
        .remote
        .put_manifest(
            &Manifest {
                refs,
                head: Some("refs/heads/main".to_string()),
            },
            etag.as_deref(),
        )
        .await
        .unwrap();
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let mut request = request;
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

async fn get(app: &Router, uri: &str, cookie: &str) -> (StatusCode, HeaderMap, String) {
    let request = Request::get(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    send(app, request).await
}

async fn post_form(app: &Router, uri: &str, cookie: &str, fields: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let request = Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    send(app, request).await
}

/// `name=value` of the `name` cookie a response sets, the part a browser
/// sends back.
fn set_cookie(headers: &HeaderMap, name: &str) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&format!("{name}=")))
        .and_then(|c| c.split(';').next())
        .unwrap_or_else(|| panic!("no {name} cookie in {headers:?}"))
        .to_string()
}

/// The CSRF token a page's form carries, read the way a browser submits it.
fn csrf_in(html: &str) -> String {
    let marker = r#"name="csrf" value=""#;
    let start = html.find(marker).unwrap_or_else(|| panic!("no csrf field: {html}")) + marker.len();
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

/// A real password login through `/_login`: fetch the form (and the cookie
/// anchoring its CSRF token), submit it, keep the session cookie it sets.
async fn login(app: &Router, username: &str, password: &str) -> String {
    let (_, headers, html) = send(app, Request::get("/_login").body(Body::empty()).unwrap()).await;
    let anchor = set_cookie(&headers, "login-csrf");
    let csrf = csrf_in(&html);
    let (status, headers, body) = post_form(
        app,
        "/_login",
        &anchor,
        &[("username", username), ("password", password), ("csrf", &csrf)],
    )
    .await;
    assert!(status.is_redirection(), "login failed ({status}): {body}");
    set_cookie(&headers, "web-session")
}

async fn create_user(ctx: &AppContext, username: &str, is_admin: bool) {
    user::create(
        ctx,
        NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: Some(PASSWORD),
            is_admin,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

struct Repo {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    ctx: Arc<AppContext>,
    resolved: ResolvedRepo,
    app: Router,
}

/// Public `team/proj` with one commit on `main`, tagged with each of `tags`.
/// Nothing is requested yet, so releases put on `resolved` afterwards are
/// what the first page load sees.
async fn repo(tags: &[&str]) -> Repo {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let resolved = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    std::fs::write(w.join("f.txt"), "hi\n").unwrap();
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "init"]).await;
    for tag in tags {
        git(w, &["tag", tag]).await;
    }
    push_all(&ctx, "team/proj", w).await;
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    Repo {
        _dirs: (root, work),
        ctx,
        resolved,
        app,
    }
}

/// `signature`: `None` unsigned, `Some(verified)` signed.
fn asset(filename: &str, sha_char: &str, signature: Option<bool>) -> AssetMeta {
    AssetMeta {
        filename: filename.to_string(),
        size: 100,
        sha256: sha_char.repeat(64),
        uploaded_at: "2024-01-01T00:00:00Z".to_string(),
        uploaded_by: "alice".to_string(),
        signature: signature.map(|_| "-----BEGIN PGP SIGNATURE-----...".to_string()),
        signature_verified: signature.unwrap_or(false),
    }
}

async fn put_release(r: &Repo, tag: &str, title: &str, is_draft: bool, assets: Vec<AssetMeta>) {
    let release = ReleaseMeta {
        tag: tag.to_string(),
        title: title.to_string(),
        notes: "**bold** notes".to_string(),
        assets,
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft,
    };
    r.resolved.remote.put_release(&release, None).await.unwrap();
}

/// A logged-in `username` holding `perm` on `team/proj`.
async fn session_with(r: &Repo, username: &str, perm: Permission) -> String {
    create_user(&r.ctx, username, false).await;
    user::set_repo_perm(&r.ctx, username, "team/proj", Some(perm), Locale::En)
        .await
        .unwrap();
    login(&r.app, username, PASSWORD).await
}

#[tokio::test]
async fn releases_list_shows_hint_when_empty() {
    let r = repo(&[]).await;
    let (status, _, html) = get(&r.app, "/team/proj?view=releases", "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("no releases yet"), "{html}");
}

#[tokio::test]
async fn releases_list_shows_each_release_and_no_form_to_an_anonymous_viewer() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "First release", false, Vec::new()).await;
    let (_, _, html) = get(&r.app, "/team/proj?view=releases", "").await;
    assert!(html.contains("First release"), "{html}");
    assert!(!html.contains(r#"action="/_releases/create/"#), "{html}");
    assert!(!html.contains("generate notes from commits"), "{html}");
}

#[tokio::test]
async fn drafts_stay_hidden_from_an_anonymous_viewer() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "Unpublished", true, Vec::new()).await;

    let (_, _, html) = get(&r.app, "/team/proj?view=releases", "").await;
    assert!(!html.contains("Unpublished"), "{html}");
    assert!(html.contains("no releases yet"), "{html}");

    // Same answer as a tag with no release at all: it doesn't even leak that
    // one is pending.
    let (status, _, html) = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains("Unpublished"), "{html}");
}

#[tokio::test]
async fn a_reader_without_write_sees_no_draft_and_no_form() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "Unpublished", true, Vec::new()).await;
    let bob = session_with(&r, "bob", Permission::Reporter).await;

    let (_, _, html) = get(&r.app, "/team/proj?view=releases", &bob).await;
    assert!(!html.contains("Unpublished"), "{html}");
    assert!(!html.contains(r#"action="/_releases/create/"#), "{html}");
    let (status, _, _) = get(&r.app, "/team/proj?view=release&tag=v1.0", &bob).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_writer_sees_drafts_and_the_write_forms() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "Unpublished", true, Vec::new()).await;
    let alice = session_with(&r, "alice", Permission::Developer).await;

    let (_, _, html) = get(&r.app, "/team/proj?view=releases", &alice).await;
    assert!(html.contains("Unpublished"), "{html}");
    assert!(html.contains(r#"action="/_releases/create/team/proj""#), "{html}");
    assert!(html.contains("generate notes from commits"), "{html}");
    let (status, _, html) = get(&r.app, "/team/proj?view=release&tag=v1.0", &alice).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(r#"action="/_releases/edit/team/proj""#), "{html}");
}

/// The generated-notes round trip re-renders the create form with whatever
/// was typed filled back in, so it must never render for someone who
/// couldn't have submitted that form.
#[tokio::test]
async fn generate_notes_never_echoes_a_prefill_to_someone_without_write() {
    let r = repo(&["v1.0"]).await;
    let leaked = [
        ("tag", "v1.0"),
        ("title", "leaked title"),
        ("notes", "leaked notes text"),
    ];

    let (status, headers, html) = post_form(&r.app, "/_releases/generate_notes/team/proj", "", &leaked).await;
    assert!(status.is_redirection(), "{status}");
    assert!(
        headers[header::LOCATION].to_str().unwrap().starts_with("/_login"),
        "{headers:?}"
    );
    assert!(!html.contains("leaked"), "{html}");

    let bob = session_with(&r, "bob", Permission::Reporter).await;
    let (_, _, page) = get(&r.app, "/team/proj?view=releases", &bob).await;
    let csrf = csrf_in(&page);
    let mut fields = leaked.to_vec();
    fields.push(("csrf", &csrf));
    let (status, _, html) = post_form(&r.app, "/_releases/generate_notes/team/proj", &bob, &fields).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!html.contains("leaked"), "{html}");
}

#[tokio::test]
async fn the_tag_field_suggests_only_tags_without_a_release_yet() {
    let r = repo(&["v0.2.0", "v0.1.0", "v0.3.0"]).await;
    put_release(&r, "v0.1.0", "Old", false, Vec::new()).await;
    let alice = session_with(&r, "alice", Permission::Developer).await;

    let (_, _, html) = get(&r.app, "/team/proj?view=releases", &alice).await;
    let start = html.find(r#"<datalist id="release-tags">"#).unwrap();
    let datalist = &html[start..start + html[start..].find("</datalist>").unwrap()];
    // Branches aren't candidates, a tag that already has a release isn't
    // either (it would only earn the "already has a release" rejection),
    // and what's left comes back sorted rather than in manifest order.
    let suggested: Vec<&str> = datalist
        .split(r#"<option value=""#)
        .skip(1)
        .map(|rest| &rest[..rest.find('"').unwrap()])
        .collect();
    assert_eq!(suggested, ["v0.2.0", "v0.3.0"]);
}

#[tokio::test]
async fn the_tag_field_suggests_nothing_when_every_tag_is_released() {
    let r = repo(&["v1.0.0"]).await;
    put_release(&r, "v1.0.0", "Only", false, Vec::new()).await;
    let alice = session_with(&r, "alice", Permission::Developer).await;
    let (_, _, html) = get(&r.app, "/team/proj?view=releases", &alice).await;
    assert!(html.contains(r#"<datalist id="release-tags"></datalist>"#), "{html}");
}

#[tokio::test]
async fn release_detail_404s_without_a_release() {
    let r = repo(&["v1.0"]).await;
    let (status, _, _) = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn release_detail_labels_the_generated_source_assets_but_not_a_regular_one() {
    let r = repo(&["v1.0"]).await;
    // Release creation generates and uploads these two in the background;
    // pre-seeded here as ordinary assets to exercise the label matching.
    let tar_gz = gitquarry_core::git::browse::archive_filename("team/proj", "v1.0", "tar.gz");
    let zip = gitquarry_core::git::browse::archive_filename("team/proj", "v1.0", "zip");
    put_release(
        &r,
        "v1.0",
        "First release",
        false,
        vec![
            asset(&tar_gz, "a", None),
            asset(&zip, "b", None),
            asset("notes.pdf", "c", None),
        ],
    )
    .await;

    let (status, _, html) = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(!html.contains("no longer exists on this repo"), "{html}");
    assert!(html.contains("Source code (tar.gz)"), "{html}");
    assert!(html.contains("Source code (zip)"), "{html}");
    // A regular asset keeps its own filename, not a generic label.
    assert!(html.contains("notes.pdf"), "{html}");
    // Real size/sha256 cells, like any other asset — not blank.
    assert!(html.contains("<td>100</td>"), "{html}");
    assert!(html.contains(&"a".repeat(6)), "{html}");
    // Downloads from stored bytes (the real asset route), not the on-demand
    // snapshot route.
    assert!(html.contains("/_raw/release_asset/"), "{html}");
    assert!(!html.contains("/_raw/snapshot/"), "{html}");
}

#[tokio::test]
async fn release_detail_badges_a_signed_asset_by_whether_it_verified() {
    let r = repo(&["v1.0"]).await;
    put_release(
        &r,
        "v1.0",
        "First release",
        false,
        vec![
            asset("asset-good.bin", "a", Some(true)),
            asset("asset-bad.bin", "b", Some(false)),
            asset("asset-plain.bin", "c", None),
        ],
    )
    .await;

    let (_, _, html) = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    let row = |name: &str| html.split("<tr>").find(|r| r.contains(name)).unwrap().to_string();
    assert!(row("asset-good.bin").contains("signature verified"), "{html}");
    assert!(row("asset-bad.bin").contains("signature not verified"), "{html}");
    assert!(!row("asset-plain.bin").contains("signature"), "{html}");
}

#[tokio::test]
async fn release_detail_notes_a_dangling_tag_without_hiding_the_release() {
    // No `v1.0` tag pushed at all — the "dangling" case.
    let r = repo(&[]).await;
    put_release(&r, "v1.0", "First release", false, vec![asset("a.bin", "a", None)]).await;

    let (status, _, html) = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("no longer exists on this repo"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
    assert!(html.contains("a.bin"), "{html}");
    // Anonymous viewer: no edit/delete/upload forms.
    assert!(!html.contains(r#"action="/_releases/edit/"#), "{html}");
    assert!(!html.contains(r#"action="/_releases/asset_upload/"#), "{html}");
    assert!(!html.contains(r#"action="/_releases/delete/"#), "{html}");
}
