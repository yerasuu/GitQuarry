mod common;

use axum::http::HeaderMap;
use axum::response::IntoResponse;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::domain::merge_request::{self, NewMergeRequest};
use gitquarry_core::domain::repo;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::merge_requests::{CommentAnchor, DiffSide, MergeStrategy, ReviewVerdict};
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::uid::Uid;
use gitquarry_i18n::Locale;
use gitquarry_webui::webui::repo::merges::handlers::{parse_verdict, resolve_target};
use gitquarry_webui::webui::repo::merges::*;
use std::sync::Arc;

/// The query params a plain merge-detail view arrives with: none. The
/// anchor params only appear when the diff's per-line link was followed.
fn no_params() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}

fn test_target(root: &std::path::Path) -> ResolvedRepo {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; SALT_LEN]).unwrap());
    let remote = Remote::new(backend, "repos/team/proj".to_string(), cipher);
    ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
    }
}

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        tracker: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn put_branch(target: &ResolvedRepo, branch: &str, sha: &str) {
    let (mut manifest, etag) = target
        .remote
        .get_manifest()
        .await
        .unwrap_or((Manifest::default(), None));
    manifest.refs.insert(format!("refs/heads/{branch}"), sha.to_string());
    target.remote.put_manifest(&manifest, etag.as_deref()).await.unwrap();
}

/// A real bare repo with `main` and a `feature` branch one commit ahead —
/// `domain::merge_request::diff` shells real `git diff`/`git log` against
/// this, unlike the manifest-only fake shas `put_branch` writes (which
/// only need to satisfy `branch_exists`'s existence check at MR-creation
/// time).
fn init_repo_with_branches(root: &std::path::Path) -> std::path::PathBuf {
    let git_dir = root.join("bare.git");
    init_mirror_with_branches(&git_dir);
    git_dir
}

/// Same as [`init_repo_with_branches`] but at an exact, caller-chosen
/// `git_dir` — `merge_request::merge`'s test needs it placed exactly
/// where `AppContext::open_mirror` would put it (`mirrors_root/<uid>`),
/// not an arbitrary subdirectory.
fn init_mirror_with_branches(git_dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_DIR", git_dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    let capture = |args: &[&str], stdin: &str| -> String {
        use std::io::Write;
        let mut child = std::process::Command::new("git")
            .env("GIT_DIR", git_dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    std::fs::create_dir_all(git_dir).unwrap();
    run(&["init", "--quiet", "--bare"]);

    let blob1 = capture(&["hash-object", "-w", "--stdin"], "hello\n");
    let tree1 = capture(&["mktree"], &format!("100644 blob {blob1}\tf.txt\n"));
    let main_sha = capture(&["commit-tree", &tree1, "-m", "initial"], "");
    run(&["update-ref", "refs/heads/main", &main_sha]);

    let blob2 = capture(&["hash-object", "-w", "--stdin"], "hello\nworld\n");
    let tree2 = capture(&["mktree"], &format!("100644 blob {blob2}\tf.txt\n"));
    let feature_sha = capture(&["commit-tree", &tree2, "-p", &main_sha, "-m", "add world"], "");
    run(&["update-ref", "refs/heads/feature", &feature_sha]);
}

#[tokio::test]
async fn merges_list_shows_hint_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let html = render_merges_list(&ctx, &target, "team/proj", "open", None, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("no merge requests yet"));
}

#[tokio::test]
async fn merges_list_shows_each_mr_linked_by_number() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();

    let html = render_merges_list(&ctx, &target, "team/proj", "open", None, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("Add widget"));
    assert!(html.contains("view=merge&amp;number=1") || html.contains("view=merge&number=1"));
}

#[tokio::test]
async fn merge_detail_404s_for_a_nonexistent_number() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    assert!(
        render_merge_detail(
            &ctx,
            &target,
            "team/proj",
            dir.path(),
            1,
            &no_params(),
            &HeaderMap::new(),
            Locale::En
        )
        .await
        .is_err()
    );
}

// Needs `gitquarry_tracker::domain::issue` to create the issue `#1` in the
// MR description resolves to — only exercised when `tracker` is on, same as
// `resolve_number_mentions`'s issue-lookup half itself.
#[cfg(feature = "tracker")]
#[tokio::test]
async fn merge_detail_links_a_hash_mention_that_resolves_to_an_issue() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let mut ctx = test_ctx(dir.path());
    let (tracker_state, link) = common::tracker::spawn_tracker(ctx.backend.clone(), ctx.cipher.clone()).await;
    ctx.tracker = Some(link);
    let ttarget = gitquarry_tracker::state::TrackerTarget {
        uid: target.uid.clone(),
    };
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    gitquarry_tracker::domain::issue::create(&tracker_state, &ttarget, "The bug", "b", "alice")
        .await
        .unwrap();
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "fixes #1",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(
        html.contains(r#"<a href="/team/proj?view=issue&amp;number=1">#1</a>"#),
        "{html}"
    );
}

#[tokio::test]
async fn merge_detail_shows_close_form_only_for_the_author_or_a_developer() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();

    // Anonymous viewer: no close form.
    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(!html.contains(r#"action="/_merges/close""#));
    assert!(html.contains("add world"), "{html}");
    assert!(html.contains("no comments yet"));
    // Anonymous viewer: no comment form either.
    assert!(!html.contains(r#"action="/_merges/comment""#));
}

#[tokio::test]
async fn merge_detail_shows_existing_comments() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();
    merge_request::add_comment(&ctx, &target, 1, "bob", "looks good to me", Locale::En)
        .await
        .unwrap();

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("looks good to me"));
    assert!(html.contains("bob"));
    assert!(!html.contains("no comments yet"));
}

#[tokio::test]
async fn a_line_comment_renders_in_the_diff_and_leaves_the_thread_alone() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();
    // `init_repo_with_branches`'s feature commit adds "world" as f.txt's
    // second line.
    merge_request::add_line_comment(
        &ctx,
        &target,
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

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    // Placed in the diff table, so the flat thread below stays empty.
    assert!(html.contains("why world?"), "{html}");
    assert!(html.contains(r#"<tr class="diff-comment">"#), "{html}");
    assert!(html.contains("no comments yet"), "{html}");
}

#[tokio::test]
async fn an_unplaceable_line_comment_falls_back_to_the_thread() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();
    merge_request::add_line_comment(
        &ctx,
        &target,
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

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("on a line that moved"), "{html}");
    assert!(!html.contains(r#"<tr class="diff-comment">"#), "{html}");
    // The thread entry says where it was meant to hang.
    assert!(html.contains("gone.txt:99"), "{html}");
    assert!(!html.contains("no comments yet"));
}

#[tokio::test]
async fn merge_detail_shows_reviews_and_their_collapsed_state() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("no reviews yet"));
    // Anonymous viewer: no review form.
    assert!(!html.contains(r#"action="/_merges/review""#));

    merge_request::add_review(&ctx, &target, 1, "bob", ReviewVerdict::Approved, "lgtm", Locale::En)
        .await
        .unwrap();
    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("approved by: bob"), "{html}");
    assert!(html.contains("lgtm"));
    assert!(!html.contains("no reviews yet"));
}

#[test]
fn an_unrecognized_verdict_falls_back_to_the_neutral_one() {
    assert_eq!(parse_verdict("approved"), ReviewVerdict::Approved);
    assert_eq!(parse_verdict("changes_requested"), ReviewVerdict::ChangesRequested);
    assert_eq!(parse_verdict("APPROVED"), ReviewVerdict::Commented);
    assert_eq!(parse_verdict(""), ReviewVerdict::Commented);
}

#[tokio::test]
async fn merge_detail_hides_the_merge_button_from_an_anonymous_viewer() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(!html.contains(r#"action="/_merges/merge""#));
}

#[tokio::test]
async fn merge_detail_shows_merge_info_once_merged() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    // `merge_request::merge` opens the target's own mirror, unlike
    // `diff`/`render_merge_detail`'s caller-supplied `git_dir` — placed
    // exactly where `AppContext::open_mirror` would put it, with a
    // manifest that matches (same technique
    // `domain::merge_request::tests::init_target_mirror` uses).
    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    init_mirror_with_branches(&git_dir);
    let (main_sha, feature_sha) = (
        gitquarry_core::git::browse::resolve_commit(&git_dir, "main")
            .await
            .unwrap()
            .unwrap(),
        gitquarry_core::git::browse::resolve_commit(&git_dir, "feature")
            .await
            .unwrap()
            .unwrap(),
    );
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), main_sha);
    manifest.refs.insert("refs/heads/feature".to_string(), feature_sha);
    target.remote.put_manifest(&manifest, None).await.unwrap();

    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();
    merge_request::merge(&ctx, &target, 1, "carol", MergeStrategy::Merge, Locale::En)
        .await
        .unwrap();

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("carol"), "{html}");
    assert!(!html.contains(r#"action="/_merges/merge""#));
}

#[tokio::test]
async fn merge_detail_shows_labels_assignees_and_requested_reviewers() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;

    let (mut config, etag) = target.remote.get_repo_config().await.unwrap();
    config.labels.push(gitquarry_core::storage::entities::Label {
        name: "bug".to_string(),
        color: "#d73a4a".to_string(),
    });
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        target.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();
    for username in ["bob", "carol"] {
        gitquarry_core::auth::domain::user::create(
            &ctx,
            gitquarry_core::auth::domain::user::NewUser {
                username,
                email: None,
                pubkey: None,
                password_plain: None,
                is_admin: false,
                repos_field: "",
                locale: Locale::En,
            },
        )
        .await
        .unwrap();
    }

    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();
    merge_request::edit_metadata(
        &ctx,
        &target,
        1,
        merge_request::MrMetadataEdit {
            title: "Add widget",
            description: "d",
            labels_csv: "bug",
            assignees_csv: "bob",
            reviewers_csv: "carol",
            milestone: None,
        },
        Locale::En,
    )
    .await
    .unwrap();

    let html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("bug"), "{html}");
    assert!(html.contains("bob"), "{html}");
    assert!(html.contains("carol"), "{html}");
}

#[tokio::test]
async fn merge_detail_shows_draft_badge_and_hides_it_once_ready() {
    let dir = tempfile::tempdir().unwrap();
    let target = test_target(dir.path());
    let ctx = test_ctx(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewMergeRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    merge_request::create(&ctx, &target, new, Locale::En).await.unwrap();
    merge_request::mark_draft(&ctx, &target, 1, Locale::En).await.unwrap();

    let draft_html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(draft_html.contains("draft"), "{draft_html}");

    merge_request::mark_ready(&ctx, &target, 1, Locale::En).await.unwrap();
    let ready_html = render_merge_detail(
        &ctx,
        &target,
        "team/proj",
        &git_dir,
        1,
        &no_params(),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(!ready_html.contains("draft"), "{ready_html}");
}

/// The bug this guards: an MR page left open across a rename must not have
/// its close/merge/comment/etc. forms resolve to whatever repo now squats
/// the name it last saw — `domain::repo::rename` frees the old name the
/// moment it lands, so it's immediately claimable by an unrelated repo.
#[tokio::test]
async fn resolve_target_rejects_a_name_reclaimed_by_a_different_repo() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());

    let original = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let original_uid = original.uid.clone();

    repo::rename(
        &ctx,
        &original,
        "team/proj",
        "team/proj-renamed",
        OwnerRef::User(Uid::from("nobody".to_string())),
        None,
        Locale::En,
    )
    .await
    .unwrap();

    // An unrelated repo claims the name "team/proj" just gave up.
    let impostor = repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(original_uid, impostor.uid);

    // A page rendered before the rename still carries `original_uid` in its
    // hidden `repo_uid` field. Resolving "team/proj" by name alone would
    // now hand the caller `impostor` -- a completely different repo.
    let err = resolve_target(&ctx, &HeaderMap::new(), "team/proj", original_uid.as_str(), Locale::En)
        .await
        .err()
        .unwrap();
    assert_eq!(err.into_response().status(), axum::http::StatusCode::NOT_FOUND);

    // The impostor's own current uid still resolves fine -- this isn't a
    // blanket "team/proj" outage, only a rejection of the stale reference.
    let (resolved, name) = resolve_target(&ctx, &HeaderMap::new(), "team/proj", impostor.uid.as_str(), Locale::En)
        .await
        .unwrap();
    assert_eq!(resolved.uid, impostor.uid);
    assert_eq!(name, "team/proj");
}
