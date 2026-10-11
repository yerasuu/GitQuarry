//! The merge-request list and detail pages, and the `/_merges/*` mutation
//! targets, driven through `webui::router` the way a browser reaches them.

use axum::Router;
use axum::http::StatusCode;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::domain::merge_request::{self, NewMergeRequest};
use gitquarry_core::domain::repo;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::merge_requests::{CommentAnchor, DiffSide, MergeStrategy, ReviewVerdict};
use gitquarry_core::storage::uid::Uid;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Page, Session, Work, git};
use std::sync::Arc;
use std::time::Duration;

/// A public `team/proj` whose `feature` branch is one commit ahead of
/// `main`, pushed into the bucket as a real pack so the server's own mirror
/// (and the `git diff`/`git log` the MR page shells) sees real history.
struct Fixture {
    _dir: tempfile::TempDir,
    work: Work,
    ctx: Arc<AppContext>,
    app: Router,
    repo: ResolvedRepo,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(AppContext {
        mirror_max_age: Duration::from_secs(1),
        ..gitquarry_test_support::ctx(dir.path())
    });
    let repo = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let work = Work::new().await;
    work.commit(&[("f.txt", "hello\n")], "initial").await;
    git(work.path(), &["checkout", "--quiet", "-b", "feature"]).await;
    work.commit(&[("f.txt", "hello\nworld\n")], "add world").await;
    work.push(&repo, "refs/heads/main").await;

    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    Fixture {
        _dir: dir,
        work,
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
    gitquarry_test_support::user(ctx, username, false).await;
    if perm.is_some() {
        user::set_repo_perm(ctx, username, "team/proj", perm, Locale::En)
            .await
            .unwrap();
    }
}

async fn login(app: &Router, username: &str) -> Session {
    Session::login(app, username).await
}

async fn detail(f: &Fixture, cookie: Option<&str>) -> String {
    let Page { status, body: html, .. } = Session::get_as(&f.app, "/team/proj?view=merge&number=1", cookie).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    html
}

#[tokio::test]
async fn merges_list_shows_hint_when_empty() {
    let f = fixture().await;
    let Page { status, body: html, .. } = Session::get_as(&f.app, "/team/proj?view=merges", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("no merge requests yet"), "{html}");
}

#[tokio::test]
async fn merges_list_shows_each_mr_linked_by_number() {
    let f = fixture().await;
    open_mr(&f).await;
    let html = Session::get_as(&f.app, "/team/proj?view=merges", None).await.body;
    assert!(html.contains("Add widget"), "{html}");
    assert!(html.contains("view=merge&amp;number=1"), "{html}");
}

#[tokio::test]
async fn merge_detail_404s_for_a_nonexistent_number() {
    let f = fixture().await;
    let status = Session::get_as(&f.app, "/team/proj?view=merge&number=1", None)
        .await
        .status;
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

    let stranger = detail(&f, Some(&login(&f.app, "bob").await.cookie)).await;
    assert!(!stranger.contains(r#"action="/_merges/close/team/proj""#), "{stranger}");
    assert!(
        stranger.contains(r#"action="/_merges/comment/team/proj""#),
        "{stranger}"
    );

    for who in ["alice", "carol"] {
        let html = detail(&f, Some(&login(&f.app, who).await.cookie)).await;
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
    let bob = login(&f.app, "bob").await;
    let uid = f.repo.uid.to_string();

    let page = bob
        .post(
            &f.app,
            "/_merges/review/team/proj",
            &[
                ("repo_uid", &uid),
                ("number", "1"),
                ("verdict", "APPROVED"),
                ("body", "shouting"),
            ],
        )
        .await;
    let status = page.status;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let html = detail(&f, None).await;
    assert!(html.contains("<strong>bob</strong> commented"), "{html}");
    assert!(!html.contains("approved by: bob"), "{html}");

    bob.post(
        &f.app,
        "/_merges/review/team/proj",
        &[("repo_uid", &uid), ("number", "1"), ("verdict", "approved")],
    )
    .await;
    assert!(detail(&f, None).await.contains("approved by: bob"));
}

#[tokio::test]
async fn a_review_without_the_sessions_csrf_token_is_rejected() {
    let f = fixture().await;
    open_mr(&f).await;
    add_user(&f.ctx, "bob", None).await;
    let bob = login(&f.app, "bob").await;
    let uid = f.repo.uid.to_string();

    for csrf in ["", "not-the-token"] {
        let forged = Session {
            cookie: bob.cookie.clone(),
            csrf: csrf.to_string(),
        };
        let status = forged
            .post(
                &f.app,
                "/_merges/review/team/proj",
                &[("repo_uid", &uid), ("number", "1"), ("verdict", "approved")],
            )
            .await
            .status;
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
    let author = detail(&f, Some(&login(&f.app, "alice").await.cookie)).await;
    assert!(!author.contains(r#"action="/_merges/merge/team/proj""#), "{author}");
    let developer = detail(&f, Some(&login(&f.app, "carol").await.cookie)).await;
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

    let html = detail(&f, Some(&login(&f.app, "carol").await.cookie)).await;
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
    let bob = login(&f.app, "bob").await;
    // What the open page's comment form will resubmit.
    let page = detail(&f, Some(&bob.cookie)).await;
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
    f.work.push(&impostor, "refs/heads/main").await;
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
        bob.post(
            &f.app,
            "/_merges/comment/team/proj",
            &[("repo_uid", uid), ("number", "1"), ("body", "hello")],
        )
        .await
        .status
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

/// An async merge swaps `#mr-actions` with the response body, so a conflict
/// has to come back as an error the toast reads, not as a redirect whose
/// full page would land inside that box.
#[tokio::test]
async fn an_async_merge_that_conflicts_is_an_error_not_a_page() {
    let f = fixture().await;
    git(f.work.path(), &["checkout", "--quiet", "main"]).await;
    f.work.commit(&[("f.txt", "hello\nmars\n")], "add mars").await;
    f.work.push(&f.repo, "refs/heads/main").await;
    open_mr(&f).await;
    add_user(&f.ctx, "carol", Some(Permission::Developer)).await;
    let carol = login(&f.app, "carol").await;

    let fields = [
        ("csrf", carol.csrf.as_str()),
        ("repo_uid", f.repo.uid.as_str()),
        ("number", "1"),
    ];
    let request = axum::http::Request::post("/_merges/merge/team/proj")
        .header(axum::http::header::COOKIE, &carol.cookie)
        .header(axum::http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-gq-async", "1")
        .body(axum::body::Body::from(
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish(),
        ))
        .unwrap();
    let response = gitquarry_test_support::send(&f.app, request).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{}", response.body);
    assert!(
        response.body.contains("conflicts with the target branch"),
        "{}",
        response.body
    );
    assert!(response.body.contains("f.txt"), "{}", response.body);
}

/// The list's checkboxes all share `name="numbers"`, so a selection arrives
/// as that key repeated once per row.
#[tokio::test]
async fn bulk_close_applies_to_every_checked_row() {
    use gitquarry_core::storage::merge_requests::MrStatus;
    let f = fixture().await;
    open_mr(&f).await;
    open_mr(&f).await;
    add_user(&f.ctx, "carol", Some(Permission::Developer)).await;
    let carol = login(&f.app, "carol").await;

    let fields = [
        ("repo_uid", f.repo.uid.as_str()),
        ("numbers", "1"),
        ("numbers", "2"),
        ("action", "close"),
    ];
    let response = carol.post(&f.app, "/_merges/bulk/team/proj", &fields).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
    for number in [1, 2] {
        let mr = merge_request::get(&f.ctx, &f.repo, number).await.unwrap().unwrap();
        assert_eq!(mr.status, MrStatus::Closed, "#{number}");
    }
}
