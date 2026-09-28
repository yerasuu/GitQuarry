//! The MR page's annotated unified diff: per-side line numbers, anchored
//! comments woven under their lines, and the per-line comment form, all as
//! `GET /{repo}?view=merge` renders them.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::domain::merge_request::{self, NewMergeRequest};
use gitquarry_core::domain::repo;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::merge_requests::{CommentAnchor, DiffSide};
use gitquarry_core::storage::remote::Manifest;
use gitquarry_i18n::Locale;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

const LEAD: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\n";

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[17u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Uploads everything reachable from `feature_sha` as one pack and points
/// `main`/`feature` at the two commits.
async fn push(repo: &ResolvedRepo, work: &Path, main_sha: &str, feature_sha: &str) {
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(&work.join(".git"), feature_sha, &[], packs.path())
        .await
        .unwrap()
        .unwrap();
    repo.remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let (_, etag) = repo.remote.get_manifest().await.unwrap();
    let manifest = Manifest {
        refs: [
            ("refs/heads/main".to_string(), main_sha.to_string()),
            ("refs/heads/feature".to_string(), feature_sha.to_string()),
        ]
        .into(),
        head: Some("refs/heads/main".to_string()),
    };
    assert_eq!(
        repo.remote.put_manifest(&manifest, etag.as_deref()).await.unwrap(),
        Cas::Ok
    );
}

/// A public `team/proj` whose `feature` branch swaps `f.txt`'s last line
/// (`gone` -> `added`, line 8 of 8) one commit ahead of `main`, so the
/// diff's only hunk starts mid-file (`@@ -5,4 +5,4 @@`), pushed into the bucket as a real pack so the server's own mirror
/// (and the `git diff`/`git log` the MR page shells) sees real history.
struct Fixture {
    _dir: tempfile::TempDir,
    ctx: Arc<AppContext>,
    app: Router,
    repo: ResolvedRepo,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    let repo = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("f.txt"), format!("{LEAD}gone\n")).unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "--quiet", "-m", "initial"]);
    let main_sha = git(&work, &["rev-parse", "HEAD"]);
    git(&work, &["checkout", "--quiet", "-b", "feature"]);
    std::fs::write(work.join("f.txt"), format!("{LEAD}added\n")).unwrap();
    git(&work, &["commit", "--quiet", "-am", "swap the last line"]);
    let feature_sha = git(&work, &["rev-parse", "HEAD"]);

    push(&repo, &work, &main_sha, &feature_sha).await;

    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    Fixture {
        _dir: dir,
        ctx,
        app,
        repo,
    }
}

async fn open_mr(f: &Fixture) {
    let new = NewMergeRequest {
        source_repo: &f.repo.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&f.ctx, &f.repo, new, Locale::En).await.unwrap();
}

async fn add_user(ctx: &AppContext, username: &str, perm: Option<Permission>) {
    user::create(
        ctx,
        user::NewUser {
            username,
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
    if perm.is_some() {
        user::set_repo_perm(ctx, username, "team/proj", perm, Locale::En)
            .await
            .unwrap();
    }
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
    // `run_server` serves with connect info; a bare `oneshot` doesn't, and
    // the git-smart-HTTP layer 500s without it.
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

/// The value of the first `<input name="csrf">` on the page, read the way a
/// browser submitting that form would.
fn csrf_from(html: &str) -> String {
    html.split(r#"name="csrf" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no csrf field: {html}"))
        .to_string()
}

/// A real password login; the returned value is the `Cookie` header that
/// carries the session from then on.
async fn login(app: &Router, username: &str) -> String {
    let (_, headers, html) = send(app, "GET", "/_login", None, None).await;
    let anchor = format!("login-csrf={}", set_cookie_value(&headers, "login-csrf"));
    let csrf = csrf_from(&html);
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

async fn detail(f: &Fixture, cookie: Option<&str>) -> String {
    let (status, _, html) = send(&f.app, "GET", "/team/proj?view=merge&number=1", cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    html
}

async fn detail_at(f: &Fixture, query: &str, cookie: Option<&str>) -> String {
    let uri = format!("/team/proj?view=merge&number=1{query}");
    let (status, _, html) = send(&f.app, "GET", &uri, cookie, None).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    html
}

/// Just the annotated diff table, so an assertion about what the diff
/// places can't be satisfied by the flat thread below it.
fn diff_table(html: &str) -> &str {
    let start = html
        .find(r#"<table class="diff-annotated">"#)
        .expect("no annotated diff");
    let len = html[start..].find("</table>").unwrap();
    &html[start..start + len]
}

async fn line_comment(f: &Fixture, author: &str, path: &str, side: DiffSide, line: u32, body: &str) {
    let anchor = CommentAnchor {
        path: path.to_string(),
        side,
        line,
    };
    merge_request::add_line_comment(&f.ctx, &f.repo, 1, author, body, anchor, Locale::En)
        .await
        .unwrap();
}

#[tokio::test]
async fn each_side_gets_its_own_line_numbering_from_the_hunk_header() {
    let f = fixture().await;
    open_mr(&f).await;
    let html = detail(&f, None).await;
    let table = diff_table(&html);
    // Numbering starts where the hunk header says, not at 1.
    assert!(
        table.contains(r#"<td class="lineno">5</td><td class="lineno">5</td>"#),
        "{table}"
    );
    assert!(!table.contains(r#"<td class="lineno">1</td>"#), "{table}");
    assert!(
        table.contains(r#"<td class="lineno">7</td><td class="lineno">7</td>"#),
        "{table}"
    );
    // The removal only advances the old side, the addition only the new.
    assert!(
        table.contains(r#"<td class="lineno">8</td><td class="lineno"></td><td class="diff-del-line">"#),
        "{table}"
    );
    assert!(
        table.contains(r#"<td class="lineno"></td><td class="lineno">8</td><td class="diff-add-line">"#),
        "{table}"
    );
}

#[tokio::test]
async fn the_per_line_link_only_shows_for_a_viewer_who_may_comment() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "dave", None).await;

    assert!(!diff_table(&detail(&f, None).await).contains("anchor_line"));

    let html = detail(&f, Some(&login(&f.app, "dave").await)).await;
    let table = diff_table(&html);
    // A context line exists on both sides and anchors to the new one; a
    // removal anchors to the old side, an addition to the new.
    for (side, line) in [("new", 7), ("old", 8), ("new", 8)] {
        let link = format!("anchor_path=f.txt&amp;anchor_side={side}&amp;anchor_line={line}\"");
        assert!(table.contains(&link), "{link}: {table}");
    }
    assert!(!table.contains("anchor_side=old&amp;anchor_line=7\""), "{table}");
}

#[tokio::test]
async fn a_comment_lands_under_the_line_it_anchors_to() {
    let f = fixture().await;
    open_mr(&f).await;
    line_comment(&f, "bob", "f.txt", DiffSide::Old, 8, "why remove this?").await;

    let html = detail(&f, None).await;
    let table = diff_table(&html);
    let comment_at = table.find("why remove this?").unwrap();
    let removed_at = table.find(">gone<").unwrap();
    let added_at = table.find(">added<").unwrap();
    assert!(removed_at < comment_at && comment_at < added_at, "{table}");
    // Placed, so the flat thread has nothing left to show.
    assert!(html.contains("no comments yet"), "{html}");
}

#[tokio::test]
async fn an_anchor_that_no_longer_matches_a_line_is_left_unplaced() {
    let f = fixture().await;
    open_mr(&f).await;
    line_comment(&f, "bob", "f.txt", DiffSide::New, 999, "stale").await;
    line_comment(&f, "bob", "other.txt", DiffSide::New, 7, "wrong file").await;

    let html = detail(&f, None).await;
    let table = diff_table(&html);
    assert!(!table.contains("stale"), "{table}");
    assert!(!table.contains("wrong file"), "{table}");
    assert!(html.contains("stale") && html.contains("wrong file"), "{html}");
}

#[tokio::test]
async fn each_hunk_gets_a_matching_toggle_and_row_group() {
    let f = fixture().await;
    open_mr(&f).await;
    let html = detail(&f, None).await;
    let table = diff_table(&html);
    assert!(
        table.contains(r#"data-toggle-rows="hunk-0" aria-expanded="true">@@ -5,4 +5,4 @@"#),
        "{table}"
    );
    assert!(table.contains(r#"<tr data-rows-group="hunk-0">"#), "{table}");
}

/// The per-line link reopens the page with `anchor_*` params; only a full,
/// usable set expands the inline form, and only on the line it names.
#[tokio::test]
async fn the_form_expands_only_on_a_well_formed_active_anchor() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "dave", None).await;
    let cookie = login(&f.app, "dave").await;
    let inline_form = r#"name="anchor_path" value="f.txt""#;

    for query in [
        "",
        "&anchor_path=f.txt",
        "&anchor_path=&anchor_line=7",
        "&anchor_path=f.txt&anchor_line=0",
        "&anchor_path=f.txt&anchor_line=x",
        "&anchor_path=f.txt&anchor_line=999",
    ] {
        let html = detail_at(&f, query, Some(&cookie)).await;
        assert!(!diff_table(&html).contains(inline_form), "{query}: {html}");
    }

    let html = detail_at(&f, "&anchor_path=f.txt&anchor_side=old&anchor_line=8", Some(&cookie)).await;
    let table = diff_table(&html);
    assert_eq!(table.matches(inline_form).count(), 1, "{table}");
    // The form's hidden fields carry the anchor back to the handler, and it
    // sits right under the line it names.
    assert!(table.contains(r#"name="anchor_side" value="old""#), "{table}");
    assert!(table.contains(r#"name="anchor_line" value="8""#), "{table}");
    assert!(table.find(">gone<").unwrap() < table.find(inline_form).unwrap());
    assert!(table.find(inline_form).unwrap() < table.find(">added<").unwrap());

    // An unrecognized side falls back to the new one, same as a missing one.
    for side in ["&anchor_side=sideways", ""] {
        let query = format!("&anchor_path=f.txt{side}&anchor_line=8");
        let html = detail_at(&f, &query, Some(&cookie)).await;
        let table = diff_table(&html);
        assert!(table.contains(r#"name="anchor_side" value="new""#), "{query}: {table}");
        assert!(table.find(">added<").unwrap() < table.find(inline_form).unwrap());
    }
}

#[tokio::test]
async fn a_placed_comments_edit_delete_controls_only_show_for_its_own_author() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "bob", None).await;
    add_user(&f.ctx, "dave", None).await;
    line_comment(&f, "bob", "f.txt", DiffSide::Old, 8, "why remove this?").await;

    for who in [None, Some("dave")] {
        let cookie = match who {
            Some(who) => Some(login(&f.app, who).await),
            None => None,
        };
        let html = detail(&f, cookie.as_deref()).await;
        let table = diff_table(&html);
        assert!(
            !table.contains(r#"action="/_merges/comment_edit/team/proj""#),
            "{who:?}: {table}"
        );
        assert!(
            !table.contains(r#"action="/_merges/comment_delete/team/proj""#),
            "{who:?}: {table}"
        );
    }

    let html = detail(&f, Some(&login(&f.app, "bob").await)).await;
    let table = diff_table(&html);
    assert!(table.contains(r#"action="/_merges/comment_edit/team/proj""#), "{table}");
    assert!(
        table.contains(r#"action="/_merges/comment_delete/team/proj""#),
        "{table}"
    );
}

#[tokio::test]
async fn a_developer_can_delete_someone_elses_placed_comment_but_not_edit_it() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "carol", Some(Permission::Developer)).await;
    line_comment(&f, "bob", "f.txt", DiffSide::Old, 8, "why remove this?").await;

    let html = detail(&f, Some(&login(&f.app, "carol").await)).await;
    let table = diff_table(&html);
    assert!(
        !table.contains(r#"action="/_merges/comment_edit/team/proj""#),
        "{table}"
    );
    assert!(
        table.contains(r#"action="/_merges/comment_delete/team/proj""#),
        "{table}"
    );
}
