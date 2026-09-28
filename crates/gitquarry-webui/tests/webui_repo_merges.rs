//! The merge-request list and detail pages, and the `/_merges/*` mutation
//! targets, driven through `webui::router` the way a browser reaches them.

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
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::merge_requests::{CommentAnchor, DiffSide, MergeStrategy, ReviewVerdict};
use gitquarry_core::storage::remote::Manifest;
use gitquarry_core::storage::uid::Uid;
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
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[13u8; s3db_crypto::SALT_LEN]).unwrap());
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

/// A public `team/proj` whose `feature` branch is one commit ahead of
/// `main`, pushed into the bucket as a real pack so the server's own mirror
/// (and the `git diff`/`git log` the MR page shells) sees real history.
struct Fixture {
    dir: tempfile::TempDir,
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
    std::fs::write(work.join("f.txt"), "hello\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "--quiet", "-m", "initial"]);
    let main_sha = git(&work, &["rev-parse", "HEAD"]);
    git(&work, &["checkout", "--quiet", "-b", "feature"]);
    std::fs::write(work.join("f.txt"), "hello\nworld\n").unwrap();
    git(&work, &["commit", "--quiet", "-am", "add world"]);
    let feature_sha = git(&work, &["rev-parse", "HEAD"]);

    push(&repo, &work, &main_sha, &feature_sha).await;

    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    Fixture { dir, ctx, app, repo }
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

#[tokio::test]
async fn merges_list_shows_hint_when_empty() {
    let f = fixture().await;
    let (status, _, html) = send(&f.app, "GET", "/team/proj?view=merges", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("no merge requests yet"), "{html}");
}

#[tokio::test]
async fn merges_list_shows_each_mr_linked_by_number() {
    let f = fixture().await;
    open_mr(&f).await;
    let (_, _, html) = send(&f.app, "GET", "/team/proj?view=merges", None, None).await;
    assert!(html.contains("Add widget"), "{html}");
    assert!(html.contains("view=merge&amp;number=1"), "{html}");
}

#[tokio::test]
async fn merge_detail_404s_for_a_nonexistent_number() {
    let f = fixture().await;
    let (status, _, _) = send(&f.app, "GET", "/team/proj?view=merge&number=1", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn merge_detail_shows_close_form_only_for_the_author_or_a_developer() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "alice", None).await;
    add_user(&f.ctx, "bob", None).await;
    add_user(&f.ctx, "carol", Some(Permission::Developer)).await;

    let html = detail(&f, None).await;
    assert!(!html.contains(r#"action="/_merges/close/team/proj""#), "{html}");
    assert!(html.contains("add world"), "{html}");
    assert!(html.contains("no comments yet"), "{html}");
    // Anonymous viewer: no comment form either.
    assert!(!html.contains(r#"action="/_merges/comment/team/proj""#), "{html}");

    let stranger = detail(&f, Some(&login(&f.app, "bob").await)).await;
    assert!(!stranger.contains(r#"action="/_merges/close/team/proj""#), "{stranger}");
    assert!(
        stranger.contains(r#"action="/_merges/comment/team/proj""#),
        "{stranger}"
    );

    for who in ["alice", "carol"] {
        let html = detail(&f, Some(&login(&f.app, who).await)).await;
        assert!(html.contains(r#"action="/_merges/close/team/proj""#), "{who}: {html}");
    }
}

#[tokio::test]
async fn merge_detail_shows_existing_comments() {
    let f = fixture().await;
    open_mr(&f).await;
    merge_request::add_comment(&f.ctx, &f.repo, 1, "bob", "looks good to me", Locale::En)
        .await
        .unwrap();

    let html = detail(&f, None).await;
    assert!(html.contains("looks good to me"), "{html}");
    assert!(html.contains("<strong>bob</strong>"), "{html}");
    assert!(!html.contains("no comments yet"), "{html}");
}

#[tokio::test]
async fn a_line_comment_renders_in_the_diff_and_leaves_the_thread_alone() {
    let f = fixture().await;
    open_mr(&f).await;
    // The feature commit adds "world" as f.txt's second line.
    merge_request::add_line_comment(
        &f.ctx,
        &f.repo,
        1,
        "bob",
        "why world?",
        CommentAnchor {
            path: "f.txt".to_string(),
            side: DiffSide::New,
            line: 2,
        },
        Locale::En,
    )
    .await
    .unwrap();

    let html = detail(&f, None).await;
    assert!(html.contains("why world?"), "{html}");
    assert!(html.contains(r#"<tr class="diff-comment">"#), "{html}");
    assert!(html.contains("no comments yet"), "{html}");
}

#[tokio::test]
async fn an_unplaceable_line_comment_falls_back_to_the_thread() {
    let f = fixture().await;
    open_mr(&f).await;
    merge_request::add_line_comment(
        &f.ctx,
        &f.repo,
        1,
        "bob",
        "on a line that moved",
        CommentAnchor {
            path: "gone.txt".to_string(),
            side: DiffSide::New,
            line: 99,
        },
        Locale::En,
    )
    .await
    .unwrap();

    let html = detail(&f, None).await;
    assert!(html.contains("on a line that moved"), "{html}");
    assert!(!html.contains(r#"<tr class="diff-comment">"#), "{html}");
    // The thread entry says where it was meant to hang.
    assert!(html.contains("gone.txt:99"), "{html}");
    assert!(!html.contains("no comments yet"), "{html}");
}

#[tokio::test]
async fn merge_detail_shows_reviews_and_their_collapsed_state() {
    let f = fixture().await;
    open_mr(&f).await;

    let html = detail(&f, None).await;
    assert!(html.contains("no reviews yet"), "{html}");
    // Anonymous viewer: no review form.
    assert!(!html.contains(r#"action="/_merges/review/team/proj""#), "{html}");

    merge_request::add_review(&f.ctx, &f.repo, 1, "bob", ReviewVerdict::Approved, "lgtm", Locale::En)
        .await
        .unwrap();
    let html = detail(&f, None).await;
    assert!(html.contains("approved by: bob"), "{html}");
    assert!(html.contains("lgtm"), "{html}");
    assert!(!html.contains("no reviews yet"), "{html}");
}

/// A hand-rolled POST can put anything in `verdict`; the worst it may get
/// is a review that says less than intended, never an approval.
#[tokio::test]
async fn an_unrecognized_verdict_falls_back_to_the_neutral_one() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "bob", None).await;
    let cookie = login(&f.app, "bob").await;
    let csrf = csrf_from(&detail(&f, Some(&cookie)).await);
    let uid = f.repo.uid.to_string();

    let (status, _, _) = send(
        &f.app,
        "POST",
        "/_merges/review/team/proj",
        Some(&cookie),
        Some(&[
            ("repo_uid", &uid),
            ("number", "1"),
            ("verdict", "APPROVED"),
            ("body", "shouting"),
            ("csrf", &csrf),
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let html = detail(&f, None).await;
    assert!(html.contains("<strong>bob</strong> commented"), "{html}");
    assert!(!html.contains("approved by: bob"), "{html}");

    send(
        &f.app,
        "POST",
        "/_merges/review/team/proj",
        Some(&cookie),
        Some(&[
            ("repo_uid", &uid),
            ("number", "1"),
            ("verdict", "approved"),
            ("csrf", &csrf),
        ]),
    )
    .await;
    assert!(detail(&f, None).await.contains("approved by: bob"));
}

#[tokio::test]
async fn a_review_without_the_sessions_csrf_token_is_rejected() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "bob", None).await;
    let cookie = login(&f.app, "bob").await;
    let uid = f.repo.uid.to_string();

    for csrf in ["", "not-the-token"] {
        let (status, _, _) = send(
            &f.app,
            "POST",
            "/_merges/review/team/proj",
            Some(&cookie),
            Some(&[
                ("repo_uid", &uid),
                ("number", "1"),
                ("verdict", "approved"),
                ("csrf", csrf),
            ]),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "csrf={csrf:?}");
    }
    assert!(detail(&f, None).await.contains("no reviews yet"));
}

#[tokio::test]
async fn merge_detail_shows_the_merge_button_only_to_a_developer() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "alice", None).await;
    add_user(&f.ctx, "carol", Some(Permission::Developer)).await;

    assert!(!detail(&f, None).await.contains(r#"action="/_merges/merge/team/proj""#));
    // Authoring the MR lets alice close it, not merge it.
    let author = detail(&f, Some(&login(&f.app, "alice").await)).await;
    assert!(!author.contains(r#"action="/_merges/merge/team/proj""#), "{author}");
    let developer = detail(&f, Some(&login(&f.app, "carol").await)).await;
    assert!(
        developer.contains(r#"action="/_merges/merge/team/proj""#),
        "{developer}"
    );
}

#[tokio::test]
async fn merge_detail_shows_merge_info_once_merged() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "carol", Some(Permission::Developer)).await;
    merge_request::merge(&f.ctx, &f.repo, 1, "carol", MergeStrategy::Merge, Locale::En)
        .await
        .unwrap();

    let html = detail(&f, Some(&login(&f.app, "carol").await)).await;
    assert!(html.contains("merged by carol"), "{html}");
    assert!(!html.contains(r#"action="/_merges/merge/team/proj""#), "{html}");
}

#[tokio::test]
async fn merge_detail_shows_labels_assignees_and_requested_reviewers() {
    let f = fixture().await;
    let (mut config, etag) = f.repo.remote.get_repo_config().await.unwrap();
    config.labels.push(gitquarry_core::storage::entities::Label {
        name: "bug".to_string(),
        color: "#d73a4a".to_string(),
    });
    gitquarry_core::storage::settings::put_repo_config(
        f.ctx.backend().as_ref(),
        f.repo.uid.as_str(),
        &config,
        etag.as_deref(),
        &f.ctx.cipher,
    )
    .await
    .unwrap();
    add_user(&f.ctx, "bob", None).await;
    add_user(&f.ctx, "carol", None).await;
    open_mr(&f).await;
    merge_request::edit_metadata(
        &f.ctx,
        &f.repo,
        1,
        merge_request::MrMetadataEdit {
            title: "Add widget",
            description: "d",
            labels_csv: "bug",
            assignees_csv: "bob",
            reviewers_csv: "carol",
        },
        Locale::En,
    )
    .await
    .unwrap();

    let html = detail(&f, None).await;
    // The label's per-repo color is inlined on the pill itself.
    assert!(
        html.contains(r#"<span class="decoration" style="border-color:#d73a4a;color:#d73a4a">bug</span>"#),
        "{html}"
    );
    assert!(html.contains("assignees: bob"), "{html}");
    assert!(html.contains("requested reviewers: carol"), "{html}");
}

#[tokio::test]
async fn merge_detail_shows_draft_badge_and_hides_it_once_ready() {
    let f = fixture().await;
    open_mr(&f).await;
    let badge = r#"<span class="badge warn">draft</span>"#;
    merge_request::mark_draft(&f.ctx, &f.repo, 1, Locale::En).await.unwrap();
    let html = detail(&f, None).await;
    assert!(html.contains(badge), "{html}");

    merge_request::mark_ready(&f.ctx, &f.repo, 1, Locale::En).await.unwrap();
    let html = detail(&f, None).await;
    assert!(!html.contains(badge), "{html}");
}

/// The bug this guards: an MR page left open across a rename must not have
/// its close/merge/comment/etc. forms resolve to whatever repo now squats
/// the name it last saw — `domain::repo::rename` frees the old name the
/// moment it lands, so it's immediately claimable by an unrelated repo.
#[tokio::test]
async fn a_form_carrying_a_stale_repo_uid_is_refused_once_the_name_is_reclaimed() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "bob", None).await;
    let cookie = login(&f.app, "bob").await;
    // What the open page's comment form will resubmit.
    let page = detail(&f, Some(&cookie)).await;
    let csrf = csrf_from(&page);
    let stale_uid = page
        .split(r#"name="repo_uid" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string();
    assert_eq!(stale_uid, f.repo.uid.as_str());

    repo::rename(
        &f.ctx,
        &f.repo,
        "team/proj",
        "team/proj-renamed",
        OwnerRef::User(Uid::from("nobody".to_string())),
        None,
        Locale::En,
    )
    .await
    .unwrap();
    // An unrelated repo claims the name "team/proj" just gave up, with an
    // MR #1 of its own for the stale form to land on.
    let impostor = repo::create(&f.ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(stale_uid, impostor.uid.as_str());
    let work = f.dir.path().join("work");
    push(
        &impostor,
        &work,
        &git(&work, &["rev-parse", "main"]),
        &git(&work, &["rev-parse", "feature"]),
    )
    .await;
    let new = NewMergeRequest {
        source_repo: &impostor.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Impostor's own",
        description: "",
        author: "mallory",
    };
    merge_request::create(&f.ctx, &impostor, new, Locale::En).await.unwrap();

    let comment = async |uid: &str| {
        send(
            &f.app,
            "POST",
            "/_merges/comment/team/proj",
            Some(&cookie),
            Some(&[("repo_uid", uid), ("number", "1"), ("body", "hello"), ("csrf", &csrf)]),
        )
        .await
        .0
    };
    assert_eq!(comment(&stale_uid).await, StatusCode::NOT_FOUND);
    assert!(
        merge_request::list_comments(&f.ctx, &impostor, 1)
            .await
            .unwrap()
            .is_empty()
    );

    // The impostor's own current uid still resolves fine -- this isn't a
    // blanket "team/proj" outage, only a rejection of the stale reference.
    assert_eq!(comment(impostor.uid.as_str()).await, StatusCode::SEE_OTHER);
    assert_eq!(
        merge_request::list_comments(&f.ctx, &impostor, 1).await.unwrap().len(),
        1
    );
}
