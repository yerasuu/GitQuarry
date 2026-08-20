use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::domain::pull_request::*;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::pull_requests::{
    CommentAnchor, DiffSide, MergeStrategy, PrReview, PrStatus, PullRequestMeta, ReviewVerdict,
};
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;

fn test_target(root: &std::path::Path) -> ResolvedRepo {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[11u8; SALT_LEN]).unwrap());
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
    let cipher = Arc::new(Cipher::derive("test passphrase", &[11u8; SALT_LEN]).unwrap());
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
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        events: Default::default(),
    }
}

async fn seed_label(ctx: &AppContext, target: &ResolvedRepo, name: &str, color: &str) {
    let (mut config, etag) = target.remote.get_repo_config().await.unwrap();
    config.labels.push(gitquarry_core::storage::entities::Label {
        name: name.to_string(),
        color: color.to_string(),
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
}

async fn seed_user(ctx: &AppContext, username: &str) {
    gitquarry_core::auth::domain::user::create(
        ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username,
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: gitquarry_i18n::Locale::En,
        },
    )
    .await
    .unwrap();
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

#[tokio::test]
async fn create_rejects_a_nonexistent_branch() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    assert!(create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.is_err());
}

#[tokio::test]
async fn create_then_get_roundtrips_and_numbers_increment() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;

    let new = |title: &'static str| NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title,
        description: "d",
        author: "alice",
    };
    let first = create(&ctx, &target, new("First"), gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    assert_eq!(first.number, 1);
    let second = create(&ctx, &target, new("Second"), gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    assert_eq!(second.number, 2);

    let fetched = get(&ctx, &target, 1).await.unwrap().unwrap();
    assert_eq!(fetched.title, "First");
    assert_eq!(fetched.status, PrStatus::Open);
}

#[tokio::test]
async fn list_returns_newest_first_and_filters_by_status() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;

    let new = |title: &'static str| NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title,
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new("First"), gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    create(&ctx, &target, new("Second"), gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    close(&ctx, &target, 1, gitquarry_i18n::Locale::Es).await.unwrap();

    let all = list(&ctx, &target, None).await.unwrap();
    assert_eq!(all.iter().map(|pr| pr.number).collect::<Vec<_>>(), vec![2, 1]);

    let open = list(&ctx, &target, Some(PrStatus::Open)).await.unwrap();
    assert_eq!(open.iter().map(|pr| pr.number).collect::<Vec<_>>(), vec![2]);
}

#[tokio::test]
async fn close_then_reopen_roundtrips_and_rejects_wrong_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();

    // Can't reopen an already-open PR.
    assert!(reopen(&ctx, &target, 1, gitquarry_i18n::Locale::Es).await.is_err());

    assert_eq!(
        close(&ctx, &target, 1, gitquarry_i18n::Locale::Es).await.unwrap(),
        Cas::Ok
    );
    assert_eq!(get(&ctx, &target, 1).await.unwrap().unwrap().status, PrStatus::Closed);

    // Can't close an already-closed PR.
    assert!(close(&ctx, &target, 1, gitquarry_i18n::Locale::Es).await.is_err());

    assert_eq!(
        reopen(&ctx, &target, 1, gitquarry_i18n::Locale::Es).await.unwrap(),
        Cas::Ok
    );
    assert_eq!(get(&ctx, &target, 1).await.unwrap().unwrap().status, PrStatus::Open);
}

#[tokio::test]
async fn transition_on_a_nonexistent_number_errs() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    assert!(close(&ctx, &target, 999, gitquarry_i18n::Locale::Es).await.is_err());
}

#[tokio::test]
async fn add_comment_rejects_a_nonexistent_number() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    assert!(
        add_comment(&ctx, &target, 999, "alice", "hi", gitquarry_i18n::Locale::Es)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn add_comment_then_list_comments_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();

    assert!(list_comments(&ctx, &target, 1).await.unwrap().is_empty());
    add_comment(&ctx, &target, 1, "bob", "looks good", gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    add_comment(&ctx, &target, 1, "alice", "thanks!", gitquarry_i18n::Locale::Es)
        .await
        .unwrap();

    let comments = list_comments(&ctx, &target, 1).await.unwrap();
    assert_eq!(
        comments.iter().map(|(_id, c)| c.body.as_str()).collect::<Vec<_>>(),
        vec!["looks good", "thanks!"]
    );
}

/// Creates PR #1 on `target`, authored by `alice`, from a manifest-only
/// `feature` -> `main` pair — enough for anything that only needs the PR
/// to exist, not a real git tree behind it.
async fn create_pr_authored_by_alice(ctx: &AppContext, target: &ResolvedRepo) {
    put_branch(target, "main", &"a".repeat(40)).await;
    put_branch(target, "feature", &"b".repeat(40)).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(ctx, target, new, gitquarry_i18n::Locale::Es).await.unwrap();
}

#[tokio::test]
async fn add_line_comment_rejects_a_malformed_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    create_pr_authored_by_alice(&ctx, &target).await;

    let anchor = |path: &str, line| CommentAnchor {
        path: path.to_string(),
        side: DiffSide::New,
        line,
    };
    for bad in [anchor("", 3), anchor("f.txt", 0)] {
        assert!(
            add_line_comment(&ctx, &target, 1, "bob", "here", bad, gitquarry_i18n::Locale::Es)
                .await
                .is_err()
        );
    }
    assert!(
        add_line_comment(
            &ctx,
            &target,
            1,
            "bob",
            "here",
            anchor("f.txt", 3),
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn a_line_comment_lands_on_the_same_thread_as_a_plain_one() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    create_pr_authored_by_alice(&ctx, &target).await;

    add_comment(&ctx, &target, 1, "bob", "plain", gitquarry_i18n::Locale::Es)
        .await
        .unwrap();
    add_line_comment(
        &ctx,
        &target,
        1,
        "carol",
        "anchored",
        CommentAnchor {
            path: "f.txt".to_string(),
            side: DiffSide::Old,
            line: 7,
        },
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();

    let comments = list_comments(&ctx, &target, 1).await.unwrap();
    assert_eq!(comments.len(), 2);
    assert!(comments[0].1.anchor.is_none());
    assert_eq!(comments[1].1.anchor.as_ref().unwrap().line, 7);
}

#[tokio::test]
async fn add_review_rejects_a_nonexistent_number() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    assert!(
        add_review(
            &ctx,
            &target,
            999,
            "bob",
            ReviewVerdict::Approved,
            "",
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn an_author_may_comment_on_but_never_approve_their_own_pr() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    create_pr_authored_by_alice(&ctx, &target).await;

    for verdict in [ReviewVerdict::Approved, ReviewVerdict::ChangesRequested] {
        assert!(
            add_review(&ctx, &target, 1, "alice", verdict, "", gitquarry_i18n::Locale::Es)
                .await
                .is_err()
        );
    }
    assert!(
        add_review(
            &ctx,
            &target,
            1,
            "alice",
            ReviewVerdict::Commented,
            "ping",
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn add_review_then_list_reviews_roundtrips_chronologically() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    create_pr_authored_by_alice(&ctx, &target).await;

    assert!(list_reviews(&ctx, &target, 1).await.unwrap().is_empty());
    add_review(
        &ctx,
        &target,
        1,
        "bob",
        ReviewVerdict::ChangesRequested,
        "needs work",
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    add_review(
        &ctx,
        &target,
        1,
        "carol",
        ReviewVerdict::Approved,
        "",
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();

    let reviews = list_reviews(&ctx, &target, 1).await.unwrap();
    assert_eq!(
        reviews.iter().map(|r| r.author.as_str()).collect::<Vec<_>>(),
        vec!["bob", "carol"]
    );
    assert_eq!(reviews[0].body, "needs work");
}

#[test]
fn review_summary_keeps_only_each_reviewers_latest_verdict() {
    let review = |author: &str, verdict| PrReview {
        author: author.to_string(),
        verdict,
        ..Default::default()
    };
    let summary = review_summary(&[
        review("bob", ReviewVerdict::ChangesRequested),
        review("carol", ReviewVerdict::Approved),
        // Bob changed his mind — only this later verdict counts.
        review("bob", ReviewVerdict::Approved),
        review("dave", ReviewVerdict::ChangesRequested),
    ]);
    assert_eq!(summary.approved_by, vec!["bob".to_string(), "carol".to_string()]);
    assert_eq!(summary.changes_requested_by, vec!["dave".to_string()]);
}

#[test]
fn a_later_neutral_review_drops_that_reviewer_from_both_sides() {
    let review = |author: &str, verdict| PrReview {
        author: author.to_string(),
        verdict,
        ..Default::default()
    };
    let summary = review_summary(&[
        review("bob", ReviewVerdict::Approved),
        review("bob", ReviewVerdict::Commented),
    ]);
    assert_eq!(summary, ReviewSummary::default());
}

/// A real bare repo with `main` and a `feature` branch one commit ahead —
/// `diff` shells real `git diff`/`git log` against this, unlike the
/// manifest-only fake shas the other tests above write (which only need
/// to satisfy `branch_exists`'s existence check at PR-creation time).
fn init_repo_with_branches(root: &std::path::Path) -> std::path::PathBuf {
    let git_dir = root.join("bare.git");
    let _ = init_mirror_with_branches(&git_dir);
    git_dir
}

/// Same as [`init_repo_with_branches`] but at an exact, caller-chosen
/// `git_dir` — `merge`'s tests need it placed exactly where
/// `AppContext::open_mirror` would put it (`mirrors_root/<uid>`), not an
/// arbitrary subdirectory. Returns `(main_sha, feature_sha)` — `feature`
/// is a descendant of `main`, so merging it in is always a trivial
/// clean merge (fast-forward-shaped), never a conflict.
fn init_mirror_with_branches(git_dir: &std::path::Path) -> (String, String) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_DIR", git_dir)
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

    (main_sha, feature_sha)
}

/// Same shape as [`init_mirror_with_branches`], plus a `CODEOWNERS` file
/// (`f.txt @dave`) present on *both* commits — `missing_required_reviewers`
/// reads it from the target branch's tree, so it has to exist there, not
/// just on `feature`.
fn init_mirror_with_branches_and_codeowners(git_dir: &std::path::Path) -> (String, String) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_DIR", git_dir)
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

    let codeowners_blob = capture(&["hash-object", "-w", "--stdin"], "f.txt @dave\n");
    let blob1 = capture(&["hash-object", "-w", "--stdin"], "hello\n");
    let tree1 = capture(
        &["mktree"],
        &format!("100644 blob {blob1}\tf.txt\n100644 blob {codeowners_blob}\tCODEOWNERS\n"),
    );
    let main_sha = capture(&["commit-tree", &tree1, "-m", "initial"], "");
    run(&["update-ref", "refs/heads/main", &main_sha]);

    let blob2 = capture(&["hash-object", "-w", "--stdin"], "hello\nworld\n");
    let tree2 = capture(
        &["mktree"],
        &format!("100644 blob {blob2}\tf.txt\n100644 blob {codeowners_blob}\tCODEOWNERS\n"),
    );
    let feature_sha = capture(&["commit-tree", &tree2, "-p", &main_sha, "-m", "add world"], "");
    run(&["update-ref", "refs/heads/feature", &feature_sha]);

    (main_sha, feature_sha)
}

/// `main` and `feature` both edit the same line of the same file,
/// independently from a shared base — merging one into the other always
/// conflicts. Returns `(main_sha, feature_sha)`.
fn init_mirror_with_conflicting_branches(git_dir: &std::path::Path) -> (String, String) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_DIR", git_dir)
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

    let base_blob = capture(&["hash-object", "-w", "--stdin"], "line1\n");
    let base_tree = capture(&["mktree"], &format!("100644 blob {base_blob}\tf.txt\n"));
    let base_sha = capture(&["commit-tree", &base_tree, "-m", "base"], "");

    let main_blob = capture(&["hash-object", "-w", "--stdin"], "main version\n");
    let main_tree = capture(&["mktree"], &format!("100644 blob {main_blob}\tf.txt\n"));
    let main_sha = capture(&["commit-tree", &main_tree, "-p", &base_sha, "-m", "main edit"], "");
    run(&["update-ref", "refs/heads/main", &main_sha]);

    let feature_blob = capture(&["hash-object", "-w", "--stdin"], "feature version\n");
    let feature_tree = capture(&["mktree"], &format!("100644 blob {feature_blob}\tf.txt\n"));
    let feature_sha = capture(
        &["commit-tree", &feature_tree, "-p", &base_sha, "-m", "feature edit"],
        "",
    );
    run(&["update-ref", "refs/heads/feature", &feature_sha]);

    (main_sha, feature_sha)
}

/// `feature` has two commits ahead of `main`, each by a different author —
/// enough commits to actually exercise the rebase strategy's per-commit
/// replay loop, and distinct authors to prove each replayed commit keeps
/// its own authorship rather than collapsing to whoever clicked merge.
/// Returns `(main_sha, first_feature_sha, second_feature_sha)`.
fn init_mirror_with_multi_commit_feature(git_dir: &std::path::Path) -> (String, String, String) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_DIR", git_dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    let capture_as = |args: &[&str], stdin: &str, author_name: &str, author_email: &str| -> String {
        use std::io::Write;
        let mut child = std::process::Command::new("git")
            .env("GIT_DIR", git_dir)
            .env("GIT_AUTHOR_NAME", author_name)
            .env("GIT_AUTHOR_EMAIL", author_email)
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

    let blob1 = capture_as(&["hash-object", "-w", "--stdin"], "hello\n", "Test", "test@example.com");
    let tree1 = capture_as(
        &["mktree"],
        &format!("100644 blob {blob1}\tf.txt\n"),
        "Test",
        "test@example.com",
    );
    let main_sha = capture_as(
        &["commit-tree", &tree1, "-m", "initial"],
        "",
        "Test",
        "test@example.com",
    );
    run(&["update-ref", "refs/heads/main", &main_sha]);

    let blob2 = capture_as(
        &["hash-object", "-w", "--stdin"],
        "hello\nalice\n",
        "Alice",
        "alice@example.com",
    );
    let tree2 = capture_as(
        &["mktree"],
        &format!("100644 blob {blob2}\tf.txt\n"),
        "Alice",
        "alice@example.com",
    );
    let f1_sha = capture_as(
        &["commit-tree", &tree2, "-p", &main_sha, "-m", "alice's commit"],
        "",
        "Alice",
        "alice@example.com",
    );

    let blob3 = capture_as(
        &["hash-object", "-w", "--stdin"],
        "hello\nalice\nbob\n",
        "Bob",
        "bob@example.com",
    );
    let tree3 = capture_as(
        &["mktree"],
        &format!("100644 blob {blob3}\tf.txt\n"),
        "Bob",
        "bob@example.com",
    );
    let f2_sha = capture_as(
        &["commit-tree", &tree3, "-p", &f1_sha, "-m", "bob's commit"],
        "",
        "Bob",
        "bob@example.com",
    );
    run(&["update-ref", "refs/heads/feature", &f2_sha]);

    (main_sha, f1_sha, f2_sha)
}

#[tokio::test]
async fn diff_returns_the_commits_and_the_diff_between_branches() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    let pr = PullRequestMeta {
        number: 1,
        source_repo: target.uid.clone(),
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        ..Default::default()
    };

    let d = diff(&ctx, &target, &git_dir, &pr).await.unwrap();
    assert_eq!(d.commits.len(), 1);
    assert_eq!(d.commits[0].subject, "add world");
    assert!(d.diff.contains("world"), "{}", d.diff);
}

/// A cross-fork pull request whose fork has no such branch — the fork was
/// deleted, or never pushed it. Renders as an empty diff rather than
/// failing: this is a read view, and a 500 there hides the pull request's
/// own title, description and discussion behind a data problem the reader
/// can do nothing about.
#[tokio::test]
async fn diff_is_empty_when_the_forks_branch_doesnt_exist() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let git_dir = init_repo_with_branches(dir.path());
    // `AppContext::open_mirror` creates an empty bare repo on demand for
    // a uid it's never seen — this fork has no `feature` branch at all.
    let fork_uid = Uid::from("team/fork".to_string());
    let pr = PullRequestMeta {
        number: 1,
        source_repo: fork_uid,
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        ..Default::default()
    };
    let d = diff(&ctx, &target, &git_dir, &pr).await.unwrap();
    assert!(d.diff.is_empty());
    assert!(d.commits.is_empty());
}

#[tokio::test]
async fn diff_supports_a_cross_fork_pr() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let target_git_dir = init_repo_with_branches(dir.path());

    // A separate fork repo (distinct uid), its mirror pre-placed exactly
    // where `AppContext::open_mirror` would put it — same technique
    // `domain::release::tests::generate_source_archives_...` uses — with
    // a manifest that matches, so `sync_bare_repo_from_s3`'s ref
    // reconciliation keeps the branch instead of deleting it as "not in
    // the manifest".
    let fork_uid = Uid::from("team/fork".to_string());
    let fork_git_dir = ctx.mirrors_root.join(fork_uid.as_str());
    std::fs::create_dir_all(&fork_git_dir).unwrap();
    let init = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_DIR", &fork_git_dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    let capture = |args: &[&str], stdin: &str| -> String {
        use std::io::Write;
        let mut child = std::process::Command::new("git")
            .env("GIT_DIR", &fork_git_dir)
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
    init(&["init", "--quiet", "--bare"]);
    // Seed the fork with the target's history first — a real fork
    // starts as a full copy of its upstream (`Remote::copy_all_to`), so
    // its new commit always has a common ancestor with the target;
    // without this, `git diff base...head` has no merge base at all.
    let fetch_status = std::process::Command::new("git")
        .env("GIT_DIR", &fork_git_dir)
        .args([
            "fetch",
            "--no-tags",
            "--end-of-options",
            target_git_dir.to_str().unwrap(),
            "refs/heads/main:refs/heads/main",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(fetch_status.success());
    let main_sha = capture(&["rev-parse", "refs/heads/main"], "");

    let blob = capture(&["hash-object", "-w", "--stdin"], "hello\nfrom the fork\n");
    let tree = capture(&["mktree"], &format!("100644 blob {blob}\tf.txt\n"));
    let fork_sha = capture(&["commit-tree", &tree, "-p", &main_sha, "-m", "fork commit"], "");
    init(&["update-ref", "refs/heads/feature", &fork_sha]);

    let fork_remote = ctx.remote_for_uid(&fork_uid);
    let mut fork_manifest = Manifest::default();
    fork_manifest.refs.insert("refs/heads/feature".to_string(), fork_sha);
    fork_remote.put_manifest(&fork_manifest, None).await.unwrap();

    let pr = PullRequestMeta {
        number: 1,
        source_repo: fork_uid,
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        ..Default::default()
    };

    let d = diff(&ctx, &target, &target_git_dir, &pr).await.unwrap();
    assert_eq!(d.commits.len(), 1);
    assert_eq!(d.commits[0].subject, "fork commit");
    assert!(d.diff.contains("from the fork"), "{}", d.diff);
}

#[tokio::test]
async fn create_rejects_a_cross_repo_pr_when_source_isnt_a_fork_of_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;

    // An unrelated repo (never forked from `target`) with its own
    // matching branch — `create` must still refuse it as a PR source.
    let other_uid = Uid::from("team/other".to_string());
    let other_remote = ctx.remote_for_uid(&other_uid);
    let mut other_manifest = Manifest::default();
    other_manifest
        .refs
        .insert("refs/heads/feature".to_string(), "b".repeat(40));
    other_remote.put_manifest(&other_manifest, None).await.unwrap();
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        other_uid.as_str(),
        &gitquarry_core::storage::entities::RepoConfig::default(),
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();

    let new = NewPullRequest {
        source_repo: &other_uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    assert!(create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.is_err());
}

/// Places `target`'s mirror exactly where `AppContext::open_mirror`
/// would put it (`mirrors_root/<uid>`) — `merge` opens its own mirror,
/// unlike `diff`'s tests which pass one in — with a manifest that
/// matches, so `sync_bare_repo_from_s3`'s ref reconciliation keeps both
/// branches instead of deleting them as "not in the manifest". Returns
/// `(main_sha, feature_sha)`.
async fn init_target_mirror(ctx: &AppContext, target: &ResolvedRepo, conflicting: bool) -> (String, String) {
    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    let (main_sha, feature_sha) = if conflicting {
        init_mirror_with_conflicting_branches(&git_dir)
    } else {
        init_mirror_with_branches(&git_dir)
    };
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), main_sha.clone());
    manifest
        .refs
        .insert("refs/heads/feature".to_string(), feature_sha.clone());
    target.remote.put_manifest(&manifest, None).await.unwrap();
    (main_sha, feature_sha)
}

/// A merged pull request whose source branch was deleted afterwards — the
/// merge form's own "delete branch" checkbox does exactly that — must still
/// render. `diff` resolves the head by *branch name* for a same-repo PR, so
/// once the branch is gone every `git diff base...head` behind the pull
/// request page failed, and the page 500'd.
#[tokio::test]
async fn diff_survives_a_source_branch_deleted_after_the_merge() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();
    merge(
        &ctx,
        &target,
        1,
        "bob",
        MergeStrategy::Merge,
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();

    // What `delete_branch=1` on the merge form does.
    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    let status = std::process::Command::new("git")
        .current_dir(&git_dir)
        .args(["branch", "-D", "feature"])
        .status()
        .unwrap();
    assert!(status.success());
    let (mut manifest, etag) = target.remote.get_manifest().await.unwrap();
    manifest.refs.remove("refs/heads/feature");
    target.remote.put_manifest(&manifest, etag.as_deref()).await.unwrap();

    let pr = get(&ctx, &target, 1).await.unwrap().unwrap();
    let d = diff(&ctx, &target, &git_dir, &pr)
        .await
        .expect("a merged pull request must still render after its branch is deleted");
    // The merge commit preserves what the pull request landed, so the diff
    // is still the real thing rather than a blank stand-in.
    assert!(d.diff.contains("f.txt"), "{}", d.diff);
}

/// The last-resort case: closed *without* merging, and the branch deleted
/// afterwards. Nothing survives to reconstruct a diff from, so the page
/// renders with an empty one rather than failing — a closed pull request's
/// title, description and discussion are still worth reading.
#[tokio::test]
async fn diff_is_empty_rather_than_an_error_when_nothing_survives() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();
    close(&ctx, &target, 1, gitquarry_i18n::Locale::Es).await.unwrap();

    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    assert!(
        std::process::Command::new("git")
            .current_dir(&git_dir)
            .args(["branch", "-D", "feature"])
            .status()
            .unwrap()
            .success()
    );

    let pr = get(&ctx, &target, 1).await.unwrap().unwrap();
    let d = diff(&ctx, &target, &git_dir, &pr).await.unwrap();
    assert!(d.diff.is_empty());
    assert!(d.commits.is_empty());
}

#[tokio::test]
async fn merge_rejects_a_nonexistent_number() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    assert!(
        merge(
            &ctx,
            &target,
            999,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn merge_rejects_a_pr_thats_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();
    close(&ctx, &target, 1, gitquarry_i18n::Locale::Es).await.unwrap();

    assert!(
        merge(
            &ctx,
            &target,
            1,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn merge_succeeds_cleanly_and_updates_the_manifest_and_pr_status() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let (main_sha, _feature_sha) = init_target_mirror(&ctx, &target, false).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();

    let merge_sha = match merge(
        &ctx,
        &target,
        1,
        "bob",
        MergeStrategy::Merge,
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap()
    {
        MergeOutcome::Merged(sha) => sha,
        MergeOutcome::Conflict(paths) => panic!("expected a clean merge, got conflicts: {paths:?}"),
    };
    assert_ne!(merge_sha, main_sha);

    let (manifest, _etag) = target.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/heads/main"), Some(&merge_sha));

    let pr = get(&ctx, &target, 1).await.unwrap().unwrap();
    assert_eq!(pr.status, PrStatus::Merged);
    assert_eq!(pr.merge_commit_sha, Some(merge_sha));
    assert_eq!(pr.merged_by, Some("bob".to_string()));
    assert!(pr.merged_at.is_some());

    // Merging an already-merged PR is rejected, same as any other
    // non-`Open` transition.
    assert!(
        merge(
            &ctx,
            &target,
            1,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn merge_rejects_until_a_required_reviewer_approves() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;

    let (mut config, etag) = target.remote.get_repo_config().await.unwrap();
    config.required_reviewers.insert("carol".to_string());
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        target.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::En).await.unwrap();

    assert!(
        merge(
            &ctx,
            &target,
            1,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::En
        )
        .await
        .is_err()
    );

    add_review(
        &ctx,
        &target,
        1,
        "carol",
        ReviewVerdict::Approved,
        "",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    assert!(matches!(
        merge(
            &ctx,
            &target,
            1,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::En
        )
        .await
        .unwrap(),
        MergeOutcome::Merged(_)
    ));
}

#[tokio::test]
async fn merge_never_requires_the_prs_own_author_to_approve_themselves() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;

    let (mut config, etag) = target.remote.get_repo_config().await.unwrap();
    // The PR's own author -- structurally can never self-approve
    // (`add_review` blocks it), so this must never block the merge.
    config.required_reviewers.insert("alice".to_string());
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        target.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::En).await.unwrap();

    assert!(matches!(
        merge(
            &ctx,
            &target,
            1,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::En
        )
        .await
        .unwrap(),
        MergeOutcome::Merged(_)
    ));
}

#[tokio::test]
async fn merge_rejects_until_the_codeowner_of_a_touched_path_approves() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    let (main_sha, feature_sha) = init_mirror_with_branches_and_codeowners(&git_dir);
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), main_sha);
    manifest.refs.insert("refs/heads/feature".to_string(), feature_sha);
    target.remote.put_manifest(&manifest, None).await.unwrap();

    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::En).await.unwrap();

    // No `RepoConfig::required_reviewers` at all -- purely CODEOWNERS
    // (`f.txt @dave`) naming a required approver for the one path this PR
    // touches.
    match merge(
        &ctx,
        &target,
        1,
        "bob",
        MergeStrategy::Merge,
        gitquarry_i18n::Locale::En,
    )
    .await
    {
        Err(e) => assert!(e.to_string().contains("dave"), "{e}"),
        Ok(_) => panic!("expected the merge to be rejected for missing dave's approval"),
    }

    add_review(
        &ctx,
        &target,
        1,
        "dave",
        ReviewVerdict::Approved,
        "",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    assert!(matches!(
        merge(
            &ctx,
            &target,
            1,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::En
        )
        .await
        .unwrap(),
        MergeOutcome::Merged(_)
    ));
}

/// The read-only probe the pull-request page renders its conflict notice
/// from: same verdict `merge` reaches, without writing anything or needing
/// merge permission — the reader seeing "this won't merge" is usually not
/// the person who can merge it.
#[tokio::test]
async fn conflicts_reports_the_same_paths_a_merge_attempt_would() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, true).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();
    let pr = get(&ctx, &target, 1).await.unwrap().unwrap();
    let mirror = ctx.open_mirror(&target).await.unwrap();

    let paths = conflicts(&ctx, &target, &mirror.path, &pr).await.unwrap();
    assert_eq!(paths, Some(vec!["f.txt".to_string()]));
}

/// A pull request that merges cleanly reports no conflicts at all, so the
/// notice never shows on a healthy PR.
#[tokio::test]
async fn conflicts_is_none_when_the_branches_merge_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();
    let pr = get(&ctx, &target, 1).await.unwrap().unwrap();
    let mirror = ctx.open_mirror(&target).await.unwrap();

    assert_eq!(conflicts(&ctx, &target, &mirror.path, &pr).await.unwrap(), None);
}

#[tokio::test]
async fn merge_reports_conflicts_without_mutating_anything() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let (main_sha, _feature_sha) = init_target_mirror(&ctx, &target, true).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();

    match merge(
        &ctx,
        &target,
        1,
        "bob",
        MergeStrategy::Merge,
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap()
    {
        MergeOutcome::Conflict(paths) => assert_eq!(paths, vec!["f.txt".to_string()]),
        MergeOutcome::Merged(sha) => panic!("expected a conflict, got a clean merge: {sha}"),
    }

    // Nothing was written: the target branch still points at its
    // pre-merge sha, and the PR is still Open.
    let (manifest, _etag) = target.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/heads/main"), Some(&main_sha));
    let pr = get(&ctx, &target, 1).await.unwrap().unwrap();
    assert_eq!(pr.status, PrStatus::Open);
}

#[tokio::test]
async fn merge_with_squash_strategy_makes_a_single_parent_commit_with_a_combined_message() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    let (main_sha, _f1_sha, f2_sha) = init_mirror_with_multi_commit_feature(&git_dir);
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), main_sha.clone());
    manifest.refs.insert("refs/heads/feature".to_string(), f2_sha.clone());
    target.remote.put_manifest(&manifest, None).await.unwrap();

    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();

    let squash_sha = match merge(
        &ctx,
        &target,
        1,
        "carol",
        MergeStrategy::Squash,
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap()
    {
        MergeOutcome::Merged(sha) => sha,
        MergeOutcome::Conflict(paths) => panic!("expected a clean merge, got conflicts: {paths:?}"),
    };

    // Single parent — the source branch's own commits never became
    // ancestors of the target, only their combined diff did.
    let parents = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .args(["log", "-1", "--format=%P", &squash_sha])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&parents.stdout).trim(), main_sha);

    let message = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .args(["log", "-1", "--format=%B", &squash_sha])
        .output()
        .unwrap();
    let message = String::from_utf8_lossy(&message.stdout);
    assert!(message.contains("Add widget (#1)"), "{message}");
    assert!(message.contains("alice's commit"), "{message}");
    assert!(message.contains("bob's commit"), "{message}");

    let (manifest, _etag) = target.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/heads/main"), Some(&squash_sha));
    assert_eq!(get(&ctx, &target, 1).await.unwrap().unwrap().status, PrStatus::Merged);
}

#[tokio::test]
async fn merge_with_rebase_strategy_replays_each_commit_linearly_and_keeps_its_author() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    let (main_sha, _f1_sha, f2_sha) = init_mirror_with_multi_commit_feature(&git_dir);
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), main_sha.clone());
    manifest.refs.insert("refs/heads/feature".to_string(), f2_sha.clone());
    target.remote.put_manifest(&manifest, None).await.unwrap();

    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "Add widget",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();

    let new_tip = match merge(
        &ctx,
        &target,
        1,
        "carol",
        MergeStrategy::Rebase,
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap()
    {
        MergeOutcome::Merged(sha) => sha,
        MergeOutcome::Conflict(paths) => panic!("expected a clean merge, got conflicts: {paths:?}"),
    };

    // Two replayed commits, still linear (one parent each), still on top
    // of the target's own pre-merge tip.
    let log = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .args(["log", "--format=%an\x1f%ae\x1f%P\x1f%s", &new_tip])
        .output()
        .unwrap();
    let lines: Vec<String> = String::from_utf8_lossy(&log.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines.len(), 3, "{lines:?}"); // bob's replayed commit, alice's replayed commit, the original main commit

    assert!(lines[0].starts_with("Bob\x1fbob@example.com\x1f"), "{}", lines[0]);
    assert!(lines[0].ends_with("bob's commit"), "{}", lines[0]);
    assert!(lines[1].starts_with("Alice\x1falice@example.com\x1f"), "{}", lines[1]);
    assert!(lines[1].ends_with("alice's commit"), "{}", lines[1]);
    // alice's replayed parent is exactly the target's own pre-merge tip.
    let alice_parent = lines[1].split('\x1f').nth(2).unwrap();
    assert_eq!(alice_parent, main_sha);

    let (manifest, _etag) = target.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/heads/main"), Some(&new_tip));
    assert_eq!(get(&ctx, &target, 1).await.unwrap().unwrap().status, PrStatus::Merged);
}

#[tokio::test]
async fn mark_merged_externally_records_status_without_touching_git() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();

    mark_merged_externally(&ctx, &target, 1, "deadbeef", Some("carol"))
        .await
        .unwrap();

    let pr = get(&ctx, &target, 1).await.unwrap().unwrap();
    assert_eq!(pr.status, PrStatus::Merged);
    assert_eq!(pr.merge_commit_sha.as_deref(), Some("deadbeef"));
    assert_eq!(pr.merged_by.as_deref(), Some("carol"));
}

#[tokio::test]
async fn mark_merged_externally_is_idempotent_once_already_merged() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(&ctx, &target, new, gitquarry_i18n::Locale::Es).await.unwrap();
    mark_merged_externally(&ctx, &target, 1, "deadbeef", None)
        .await
        .unwrap();

    // Re-observing the same upstream merge on a later sync tick must
    // not error just because it already landed.
    assert!(mark_merged_externally(&ctx, &target, 1, "deadbeef", None).await.is_ok());
}

#[tokio::test]
async fn mark_merged_externally_rejects_a_nonexistent_number() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    assert!(
        mark_merged_externally(&ctx, &target, 999, "deadbeef", None)
            .await
            .is_err()
    );
}

/// Assumes `main`/`feature` already exist on `target` — either via
/// [`put_branch`] (a metadata-only PR, no real mirror) or
/// [`init_target_mirror`] (a real mirror, needed for anything that
/// actually merges).
async fn create_test_pr(ctx: &AppContext, target: &ResolvedRepo) -> PullRequestMeta {
    let new = NewPullRequest {
        source_repo: &target.uid,
        source_branch: "feature",
        target_branch: "main",
        title: "t",
        description: "d",
        author: "alice",
    };
    create(ctx, target, new, gitquarry_i18n::Locale::En).await.unwrap()
}

#[tokio::test]
async fn edit_metadata_rejects_an_unknown_label() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;

    let result = edit_metadata(
        &ctx,
        &target,
        pr.number,
        PrMetadataEdit {
            title: "t",
            description: "d",
            labels_csv: "nonexistent",
            assignees_csv: "",
            reviewers_csv: "",
        },
        gitquarry_i18n::Locale::En,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_metadata_rejects_an_unknown_assignee() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;

    let result = edit_metadata(
        &ctx,
        &target,
        pr.number,
        PrMetadataEdit {
            title: "t",
            description: "d",
            labels_csv: "",
            assignees_csv: "nobody",
            reviewers_csv: "",
        },
        gitquarry_i18n::Locale::En,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_metadata_rejects_an_unknown_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;

    let result = edit_metadata(
        &ctx,
        &target,
        pr.number,
        PrMetadataEdit {
            title: "t",
            description: "d",
            labels_csv: "",
            assignees_csv: "",
            reviewers_csv: "nobody",
        },
        gitquarry_i18n::Locale::En,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_metadata_updates_labels_assignees_and_reviewers() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    seed_label(&ctx, &target, "bug", "#d73a4a").await;
    seed_user(&ctx, "bob").await;
    seed_user(&ctx, "carol").await;
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;

    let cas = edit_metadata(
        &ctx,
        &target,
        pr.number,
        PrMetadataEdit {
            title: "t",
            description: "d",
            labels_csv: "bug",
            assignees_csv: "bob",
            reviewers_csv: "carol",
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert_eq!(cas, Cas::Ok);

    let updated = get(&ctx, &target, pr.number).await.unwrap().unwrap();
    assert_eq!(updated.labels, vec!["bug".to_string()]);
    assert_eq!(updated.assignees, vec!["bob".to_string()]);
    assert_eq!(updated.requested_reviewers, vec!["carol".to_string()]);
}

#[tokio::test]
async fn mark_draft_then_mark_ready_roundtrips_and_rejects_wrong_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;
    assert!(!pr.is_draft);

    // Can't mark ready a PR that's already not a draft.
    assert!(
        mark_ready(&ctx, &target, pr.number, gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );

    assert_eq!(
        mark_draft(&ctx, &target, pr.number, gitquarry_i18n::Locale::En)
            .await
            .unwrap(),
        Cas::Ok
    );
    assert!(get(&ctx, &target, pr.number).await.unwrap().unwrap().is_draft);

    // Can't mark draft a PR that's already a draft.
    assert!(
        mark_draft(&ctx, &target, pr.number, gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );

    assert_eq!(
        mark_ready(&ctx, &target, pr.number, gitquarry_i18n::Locale::En)
            .await
            .unwrap(),
        Cas::Ok
    );
    assert!(!get(&ctx, &target, pr.number).await.unwrap().unwrap().is_draft);
}

#[tokio::test]
async fn merge_rejects_a_draft_pr() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let pr = create_test_pr(&ctx, &target).await;
    mark_draft(&ctx, &target, pr.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();

    assert!(
        merge(
            &ctx,
            &target,
            pr.number,
            "bob",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::En
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn merge_rejects_a_pr_with_pending_changes_requested() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    init_target_mirror(&ctx, &target, false).await;
    let pr = create_test_pr(&ctx, &target).await;
    add_review(
        &ctx,
        &target,
        pr.number,
        "bob",
        ReviewVerdict::ChangesRequested,
        "please fix this",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    assert!(
        merge(
            &ctx,
            &target,
            pr.number,
            "carol",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::En
        )
        .await
        .is_err()
    );

    // A later approval from the same reviewer supersedes their own
    // rejection (only the latest verdict per reviewer counts), unblocking
    // the merge.
    add_review(
        &ctx,
        &target,
        pr.number,
        "bob",
        ReviewVerdict::Approved,
        "actually fine",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(
        merge(
            &ctx,
            &target,
            pr.number,
            "carol",
            MergeStrategy::Merge,
            gitquarry_i18n::Locale::En
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn edit_comment_updates_the_body_but_rejects_a_non_author() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;
    add_comment(&ctx, &target, pr.number, "alice", "first", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let (comment_id, _) = &list_comments(&ctx, &target, pr.number).await.unwrap()[0];

    assert!(
        edit_comment(
            &ctx,
            &target,
            pr.number,
            comment_id,
            "bob",
            "hijacked",
            gitquarry_i18n::Locale::En,
        )
        .await
        .is_err()
    );

    assert_eq!(
        edit_comment(
            &ctx,
            &target,
            pr.number,
            comment_id,
            "alice",
            "edited",
            gitquarry_i18n::Locale::En,
        )
        .await
        .unwrap(),
        Cas::Ok
    );
    let comments = list_comments(&ctx, &target, pr.number).await.unwrap();
    assert_eq!(comments[0].1.body, "edited");
}

#[tokio::test]
async fn delete_comment_removes_it_but_rejects_a_non_author() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;
    add_comment(&ctx, &target, pr.number, "alice", "first", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let (comment_id, _) = &list_comments(&ctx, &target, pr.number).await.unwrap()[0];

    assert!(
        delete_comment(
            &ctx,
            &target,
            pr.number,
            comment_id,
            "bob",
            false,
            gitquarry_i18n::Locale::En,
        )
        .await
        .is_err()
    );

    delete_comment(
        &ctx,
        &target,
        pr.number,
        comment_id,
        "alice",
        false,
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(list_comments(&ctx, &target, pr.number).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_comment_allows_a_moderator_to_remove_someone_elses() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let pr = create_test_pr(&ctx, &target).await;
    add_comment(&ctx, &target, pr.number, "alice", "first", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let (comment_id, _) = &list_comments(&ctx, &target, pr.number).await.unwrap()[0];

    // Not the author, but the caller vouches `bob` is Developer+ on the repo.
    delete_comment(
        &ctx,
        &target,
        pr.number,
        comment_id,
        "bob",
        true,
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(list_comments(&ctx, &target, pr.number).await.unwrap().is_empty());
}

/// `feature`'s tip carries a subject and a body, so a submit that leaves
/// both fields blank has something to fall back to.
fn init_target_mirror_with_a_message(git_dir: &std::path::Path, subject: &str, body: &str) -> String {
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
    let (main_sha, _feature_sha) = init_mirror_with_branches(git_dir);

    let blob = capture(&["hash-object", "-w", "--stdin"], "tip\n");
    let tree = capture(&["mktree"], &format!("100644 blob {blob}\tf.txt\n"));
    let message = if body.is_empty() {
        subject.to_string()
    } else {
        format!("{subject}\n\n{body}")
    };
    let tip = capture(&["commit-tree", &tree, "-p", &main_sha, "-m", &message], "");
    let status = std::process::Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(["update-ref", "refs/heads/feature", &tip])
        .status()
        .unwrap();
    assert!(status.success());
    tip
}

async fn target_with_message(ctx: &AppContext, target: &ResolvedRepo, subject: &str, body: &str) {
    let git_dir = ctx.mirrors_root.join(target.uid.as_str());
    let tip = init_target_mirror_with_a_message(&git_dir, subject, body);
    let mut manifest = Manifest::default();
    let main_sha = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .args(["rev-parse", "refs/heads/main"])
        .output()
        .unwrap();
    let main_sha = String::from_utf8(main_sha.stdout).unwrap().trim().to_string();
    manifest.refs.insert("refs/heads/main".to_string(), main_sha);
    manifest.refs.insert("refs/heads/feature".to_string(), tip);
    target.remote.put_manifest(&manifest, None).await.unwrap();
}

#[tokio::test]
async fn create_falls_back_to_the_tip_commits_subject_and_body() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    target_with_message(
        &ctx,
        &target,
        "add the widget",
        "Because the old one broke.\nSecond line.",
    )
    .await;

    let pr = create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "",
            description: "",
            author: "alice",
        },
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    assert_eq!(pr.title, "add the widget");
    assert_eq!(pr.description, "Because the old one broke.\nSecond line.");
}

/// Only the blank field is filled in — a submitted title still wins.
#[tokio::test]
async fn create_keeps_whatever_the_submitter_actually_typed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    target_with_message(&ctx, &target, "add the widget", "commit body").await;

    let pr = create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "my own title",
            description: "",
            author: "alice",
        },
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    assert_eq!(pr.title, "my own title");
    assert_eq!(pr.description, "commit body");
}

/// A tip commit with no body leaves the description empty — an empty
/// description is an ordinary PR, unlike an empty title.
#[tokio::test]
async fn create_leaves_the_description_empty_when_the_commit_has_no_body() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    target_with_message(&ctx, &target, "just a subject", "").await;

    let pr = create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "",
            description: "",
            author: "alice",
        },
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    assert_eq!(pr.title, "just a subject");
    assert_eq!(pr.description, "");
}
