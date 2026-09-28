use gitquarry_core::app::{AppContext, Locks};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_mirror_external::engine::*;
use gitquarry_mirror_external::{access, domain};
use std::sync::Arc;
use std::time::Duration;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    });
    let cipher = Arc::new(Cipher::derive("test passphrase", &[11u8; gitquarry_core::crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend: Arc::new(backend) as Arc<dyn Backend>,
        locks: Locks::default(),
        cipher,
        mirror_max_age: Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// Every build allows the forges it has a client for, but a test's
/// stand-in target isn't one of those — so a test that stores a mirror adds
/// its host the same way an admin adds a self-hosted forge.
async fn allow_mirror_host(ctx: &AppContext, host: &str) {
    let (mut settings, etag) = gitquarry_core::storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher)
        .await
        .unwrap();
    settings.mirror_extra_hosts.push(host.to_string());
    gitquarry_core::storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher)
        .await
        .unwrap();
}

/// A tiny real bare repo with `a` (root commit) as the parent of both
/// `b` and `c` (sibling children, so `b`/`c` are mutually non-ancestor —
/// a genuine divergence) — everything [`plan_ref_sync`]'s test cases
/// below need real objects/ancestry for.
async fn ancestry_fixture() -> (tempfile::TempDir, String, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("repo.git");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet", "--bare"])
            .current_dir(&bare)
            .status()
            .unwrap()
            .success()
    );

    let work = tempfile::tempdir().unwrap();
    async fn run_git(dir: &std::path::Path, args: &[&str]) {
        assert!(
            tokio::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .status()
                .await
                .unwrap()
                .success()
        );
    }
    async fn commit(dir: &std::path::Path, file: &str, content: &str, msg: &str) -> String {
        std::fs::write(dir.join(file), content).unwrap();
        run_git(dir, &["add", "-A"]).await;
        run_git(
            dir,
            &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", msg],
        )
        .await;
        let out = tokio::process::Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    run_git(work.path(), &["init", "-q"]).await;
    let a = commit(work.path(), "f.txt", "a\n", "a").await;
    let b = commit(work.path(), "f.txt", "b\n", "b").await;
    run_git(work.path(), &["checkout", "-q", &a]).await;
    let c = commit(work.path(), "g.txt", "c\n", "c").await;

    async fn push_as(work: &std::path::Path, bare: &std::path::Path, sha: &str, refname: &str) {
        let bare = bare.to_string_lossy().to_string();
        let refspec = format!("{sha}:{refname}");
        assert!(
            tokio::process::Command::new("git")
                .current_dir(work)
                .args(["push", "-q", &bare, &refspec])
                .status()
                .await
                .unwrap()
                .success()
        );
    }
    push_as(work.path(), &bare, &a, "refs/heads/x").await;
    push_as(work.path(), &bare, &b, "refs/heads/y").await;
    push_as(work.path(), &bare, &c, "refs/heads/z").await;
    (dir, a, b, c)
}

#[tokio::test]
async fn plan_ref_sync_pushes_a_brand_new_local_only_ref() {
    let (dir, _a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(&bare, Some(&b), None, None, access::SyncDirection::Bidirectional)
        .await
        .unwrap();
    assert_eq!(action, Some(RefAction::Push(b)));
}

#[tokio::test]
async fn plan_ref_sync_pulls_a_brand_new_remote_only_ref() {
    let (dir, _a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(&bare, None, Some(&b), None, access::SyncDirection::Bidirectional)
        .await
        .unwrap();
    assert_eq!(action, Some(RefAction::Pull(b)));
}

#[tokio::test]
async fn plan_ref_sync_records_an_already_equal_ref_with_no_baseline_yet() {
    let (dir, _a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(&bare, Some(&b), Some(&b), None, access::SyncDirection::Bidirectional)
        .await
        .unwrap();
    assert_eq!(action, Some(RefAction::Record(b)));
}

#[tokio::test]
async fn plan_ref_sync_does_nothing_once_already_recorded_and_unchanged() {
    let (dir, _a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(
        &bare,
        Some(&b),
        Some(&b),
        Some(&b),
        access::SyncDirection::Bidirectional,
    )
    .await
    .unwrap();
    assert_eq!(action, None);
}

#[tokio::test]
async fn plan_ref_sync_pushes_when_only_local_moved_since_last_sync() {
    let (dir, a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(
        &bare,
        Some(&b),
        Some(&a),
        Some(&a),
        access::SyncDirection::Bidirectional,
    )
    .await
    .unwrap();
    assert_eq!(action, Some(RefAction::Push(b)));
}

#[tokio::test]
async fn plan_ref_sync_pulls_when_only_remote_moved_since_last_sync() {
    let (dir, a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(
        &bare,
        Some(&a),
        Some(&b),
        Some(&a),
        access::SyncDirection::Bidirectional,
    )
    .await
    .unwrap();
    assert_eq!(action, Some(RefAction::Pull(b)));
}

#[tokio::test]
async fn plan_ref_sync_fast_forward_pulls_when_both_moved_but_remote_is_ahead() {
    let (dir, a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    // No `last_synced` baseline at all — both sides count as "moved",
    // but `a` is a real ancestor of `b`, so this is still a clean
    // fast-forward, not a divergence.
    let action = plan_ref_sync(&bare, Some(&a), Some(&b), None, access::SyncDirection::Bidirectional)
        .await
        .unwrap();
    assert_eq!(action, Some(RefAction::Pull(b)));
}

#[tokio::test]
async fn plan_ref_sync_fast_forward_pushes_when_both_moved_but_local_is_ahead() {
    let (dir, a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(&bare, Some(&b), Some(&a), None, access::SyncDirection::Bidirectional)
        .await
        .unwrap();
    assert_eq!(action, Some(RefAction::Push(b)));
}

#[tokio::test]
async fn plan_ref_sync_diverges_when_neither_side_is_an_ancestor_of_the_other() {
    let (dir, _a, b, c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    let action = plan_ref_sync(&bare, Some(&b), Some(&c), None, access::SyncDirection::Bidirectional)
        .await
        .unwrap();
    assert_eq!(action, Some(RefAction::Diverged));
}

#[tokio::test]
async fn plan_ref_sync_diverges_rather_than_replaying_a_rewound_remote() {
    let (dir, _a, b, c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    // The remote rewrote its history (`b` -> `c`, siblings) while the local
    // side sat exactly where the last sync left it. "Only the remote moved"
    // used to be enough to pull it, silently discarding `b` locally.
    let action = plan_ref_sync(
        &bare,
        Some(&b),
        Some(&c),
        Some(&b),
        access::SyncDirection::Bidirectional,
    )
    .await
    .unwrap();
    assert_eq!(action, Some(RefAction::Diverged));
}

#[tokio::test]
async fn plan_ref_sync_never_resurrects_a_ref_deleted_on_either_side() {
    let (dir, _a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    // Deleted locally: the remote still advertises the sha this mirror last
    // synced, so pulling it back would undo the deletion every tick.
    assert_eq!(
        plan_ref_sync(&bare, None, Some(&b), Some(&b), access::SyncDirection::Bidirectional)
            .await
            .unwrap(),
        Some(RefAction::DeletedOneSide)
    );
    // Deleted on the external forge: same, mirrored.
    assert_eq!(
        plan_ref_sync(&bare, Some(&b), None, Some(&b), access::SyncDirection::Bidirectional)
            .await
            .unwrap(),
        Some(RefAction::DeletedOneSide)
    );
}

#[tokio::test]
async fn plan_ref_sync_respects_push_only_and_pull_only_directions() {
    let (dir, _a, b, _c) = ancestry_fixture().await;
    let bare = dir.path().join("repo.git");
    assert_eq!(
        plan_ref_sync(&bare, None, Some(&b), None, access::SyncDirection::PushOnly)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        plan_ref_sync(&bare, Some(&b), None, None, access::SyncDirection::PullOnly)
            .await
            .unwrap(),
        None
    );
}

/// A plain (non-`gitquarry`) bare repo with one commit already on
/// `refs/heads/main` — the "external forge already has something"
/// half of a pull/divergence fixture.
async fn plain_bare_repo_with_commit(content: &str) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("target.git");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(
        tokio::process::Command::new("git")
            .args(["init", "--quiet", "--bare"])
            .current_dir(&bare)
            .status()
            .await
            .unwrap()
            .success()
    );

    let work = tempfile::tempdir().unwrap();
    assert!(
        tokio::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(work.path())
            .status()
            .await
            .unwrap()
            .success()
    );
    std::fs::write(work.path().join("f.txt"), content).unwrap();
    assert!(
        tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(work.path())
            .status()
            .await
            .unwrap()
            .success()
    );
    assert!(
        tokio::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "c"])
            .current_dir(work.path())
            .status()
            .await
            .unwrap()
            .success()
    );
    let out = tokio::process::Command::new("git")
        .current_dir(work.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .unwrap();
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        tokio::process::Command::new("git")
            .current_dir(work.path())
            .args(["push", "-q", &bare.to_string_lossy(), "HEAD:refs/heads/main"])
            .status()
            .await
            .unwrap()
            .success()
    );
    (dir, bare, sha)
}

/// Real `git push` mechanics end to end (no mocked git anywhere), but
/// against a plain bare repo standing in for the external forge —
/// deliberately *not* a second `gitquarry`-managed repo behind
/// `webui::run_server`: `git::http::handler` unconditionally re-syncs
/// the target mirror from S3 right after every receive-pack response
/// (see its doc comment — "the pre-receive hook already pushed to the
/// bucket"), which assumes the *real* `gitquarry pre-receive-hook`
/// subcommand ran and wrote that S3 state. No test in this codebase
/// points `AppContext::exe_path` at a real invocable binary (every
/// `test_ctx`-style helper uses a nonexistent placeholder path — a
/// `cargo test` unit-test binary has no `CARGO_BIN_EXE_*` for its own
/// crate to substitute one), so exercising that hook for real is out of
/// scope here; this test instead verifies what this module actually
/// owns — the ref-diff decision and CAS bookkeeping — against a target
/// git genuinely has to negotiate and accept a push into.
#[tokio::test]
async fn sync_mirror_once_pushes_a_changed_local_ref_to_the_external_target() {
    use gitquarry_core::storage::remote::Manifest;
    use std::collections::BTreeMap;
    use tokio::process::Command;

    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));

    // The "external forge" stand-in: a plain bare repo, empty until the
    // push lands — no gitquarry machinery involved on this side at all.
    let target_dir = tempfile::tempdir().unwrap();
    let target_path = target_dir.path().join("target.git");
    std::fs::create_dir_all(&target_path).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet", "--bare"])
            .current_dir(&target_path)
            .status()
            .unwrap()
            .success()
    );

    // Source repo — has one real commit, built the same "scratch working
    // tree -> pack -> upload_pack + put_manifest" way
    // `mirror::tests::open_remote_mirror_fetches_a_real_peers_repo_over_git_smart_http`
    // builds instance B's content, then pulled onto local disk via
    // `open_mirror` so `sync_mirror_once` has a real bare repo to push
    // from.
    let source = ctx.create_repo("alice/source").await.unwrap().unwrap();
    let work = tempfile::tempdir().unwrap();
    async fn run_git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git").current_dir(dir).args(args).status().await.unwrap();
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }
    run_git(work.path(), &["init", "-q"]).await;
    std::fs::write(work.path().join("README.md"), b"hello from source\n").unwrap();
    run_git(work.path(), &["add", "-A"]).await;
    run_git(
        work.path(),
        &[
            "-c",
            "user.email=test@test",
            "-c",
            "user.name=test",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    )
    .await;
    let sha_out = Command::new("git")
        .current_dir(work.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .unwrap();
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

    let git_dir = work.path().join(".git");
    let pack_dir = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(&git_dir, &sha, &[], pack_dir.path())
        .await
        .unwrap()
        .unwrap();
    source
        .remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    source
        .remote
        .put_manifest(
            &Manifest {
                refs: BTreeMap::from([("refs/heads/main".to_string(), sha.clone())]),
                head: Some("refs/heads/main".to_string()),
            },
            None,
        )
        .await
        .unwrap();
    // Scoped: `open_mirror` holds this repo's lock for as long as the
    // guard lives, and `sync_mirror_once` below re-opens the same
    // mirror (its own lock acquisition) — keeping this one alive past
    // that call would self-deadlock.
    {
        ctx.open_mirror(&source).await.unwrap();
    }

    // Created via the real validated path (`domain::create` ->
    // `access::set_external_mirror`, http(s)-only) to prove that part
    // works too, then `remote_url` is overridden in-memory to the real
    // bare repo above: `sync_mirror_once` takes whatever `ExternalMirror`
    // it's handed and never re-validates/re-fetches it, so this only
    // swaps what the *push itself* targets, not what got persisted.
    allow_mirror_host(&ctx, "example.invalid").await;
    let mut mirror = domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://example.invalid/acme/target.git".to_string(),
            token: "s3cr3t-token".to_string(),
            enabled: true,
            label: "test target".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    mirror.remote_url = target_path.to_string_lossy().to_string();

    let report = sync_mirror_once(&ctx, &source, &mirror).await.unwrap();
    assert_eq!(report.pushed, 1);
    assert_eq!(report.failure_summary(), None);

    let target_refs = gitquarry_mirror_external::git_remote::list_refs_with_sha(&target_path)
        .await
        .unwrap();
    assert_eq!(target_refs.get("refs/heads/main"), Some(&sha));

    let (persisted, _) = domain::get(&ctx, &source.uid, &mirror.id).await.unwrap().unwrap();
    assert_eq!(persisted.last_synced.get("refs/heads/main"), Some(&sha));
    assert_eq!(
        persisted.ref_status.get("refs/heads/main"),
        Some(&access::RefSyncStatus::InSync)
    );
}

/// Pull direction's counterpart to the push test above: the external
/// target already has a ref the local repo has never seen, and
/// `sync_mirror_once` has to bring it in — real `git fetch` +
/// `update_ref`, not the push side of [`plan_ref_sync`] at all.
#[tokio::test]
async fn sync_mirror_once_pulls_a_new_ref_from_the_external_target() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));

    let (_target_dir, target_path, sha) = plain_bare_repo_with_commit("hello from target\n").await;

    // Source repo — created but never given any content, so its local
    // mirror starts with zero refs.
    let source = ctx.create_repo("alice/source-pull").await.unwrap().unwrap();
    {
        ctx.open_mirror(&source).await.unwrap();
    }

    allow_mirror_host(&ctx, "example.invalid").await;
    let mut mirror = domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://example.invalid/acme/target.git".to_string(),
            token: "s3cr3t-token".to_string(),
            enabled: true,
            label: "test target".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    mirror.remote_url = target_path.to_string_lossy().to_string();

    let report = sync_mirror_once(&ctx, &source, &mirror).await.unwrap();
    assert_eq!(report.pulled, 1);
    assert_eq!(report.failure_summary(), None);

    let source_path = ctx.mirrors_root.join(source.uid.as_str());
    let source_refs = gitquarry_mirror_external::git_remote::list_refs_with_sha(&source_path)
        .await
        .unwrap();
    assert_eq!(source_refs.get("refs/heads/main"), Some(&sha));

    // The pull has to be real, not just a hit on the local mirror
    // cache: S3's own manifest (the actual source of truth) must carry
    // the pulled ref too, or `open_mirror` re-syncing from it —
    // *anywhere*, including this same repo's very next web/git request —
    // would silently wipe the ref straight back out.
    let (manifest, _etag) = source.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/heads/main"), Some(&sha));

    // Pulled packs count toward the owner's storage total exactly like a
    // push's do — otherwise the quota check on the pull path can never
    // trip, however much arrives through the mirror.
    let (config, _) = source.remote.get_repo_config().await.unwrap();
    assert!(
        config.pack_bytes > 0,
        "the pulled pack wasn't counted toward the repo's stored bytes"
    );

    // Reproduces the regression directly: re-opening the mirror (what
    // every subsequent request against this repo does) must not lose
    // what was just pulled.
    ctx.open_mirror(&source).await.unwrap();
    let source_refs_after_reopen = gitquarry_mirror_external::git_remote::list_refs_with_sha(&source_path)
        .await
        .unwrap();
    assert_eq!(source_refs_after_reopen.get("refs/heads/main"), Some(&sha));

    let (persisted, _) = domain::get(&ctx, &source.uid, &mirror.id).await.unwrap().unwrap();
    assert_eq!(persisted.last_synced.get("refs/heads/main"), Some(&sha));
    assert_eq!(
        persisted.ref_status.get("refs/heads/main"),
        Some(&access::RefSyncStatus::InSync)
    );
}

/// Two unrelated commits on the same ref name, no shared history at
/// all — `sync_mirror_once` must record `Diverged` and leave *both*
/// sides exactly as they were, never forcing either one.
#[tokio::test]
async fn sync_mirror_once_marks_a_genuine_divergence_without_forcing_either_side() {
    use gitquarry_core::storage::remote::Manifest;
    use std::collections::BTreeMap;
    use tokio::process::Command;

    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));

    let (_target_dir, target_path, target_sha) = plain_bare_repo_with_commit("hello from target\n").await;

    let source = ctx.create_repo("alice/source-diverge").await.unwrap().unwrap();
    let work = tempfile::tempdir().unwrap();
    async fn run_git(dir: &std::path::Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(dir)
                .args(args)
                .status()
                .await
                .unwrap()
                .success()
        );
    }
    run_git(work.path(), &["init", "-q"]).await;
    std::fs::write(work.path().join("g.txt"), b"hello from source\n").unwrap();
    run_git(work.path(), &["add", "-A"]).await;
    run_git(
        work.path(),
        &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "s"],
    )
    .await;
    let source_sha_out = Command::new("git")
        .current_dir(work.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .unwrap();
    let source_sha = String::from_utf8_lossy(&source_sha_out.stdout).trim().to_string();

    let git_dir = work.path().join(".git");
    let pack_dir = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(&git_dir, &source_sha, &[], pack_dir.path())
        .await
        .unwrap()
        .unwrap();
    source
        .remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    source
        .remote
        .put_manifest(
            &Manifest {
                refs: BTreeMap::from([("refs/heads/main".to_string(), source_sha.clone())]),
                head: Some("refs/heads/main".to_string()),
            },
            None,
        )
        .await
        .unwrap();
    {
        ctx.open_mirror(&source).await.unwrap();
    }

    allow_mirror_host(&ctx, "example.invalid").await;
    let mut mirror = domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://example.invalid/acme/target.git".to_string(),
            token: "s3cr3t-token".to_string(),
            enabled: true,
            label: "test target".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    mirror.remote_url = target_path.to_string_lossy().to_string();

    let report = sync_mirror_once(&ctx, &source, &mirror).await.unwrap();
    // A divergence is not an `Err`, but it is a failure to sync.
    assert_eq!(report.diverged, vec!["refs/heads/main".to_string()]);
    assert!(report.failure_summary().is_some());

    let target_refs = gitquarry_mirror_external::git_remote::list_refs_with_sha(&target_path)
        .await
        .unwrap();
    assert_eq!(target_refs.get("refs/heads/main"), Some(&target_sha));
    let source_path = ctx.mirrors_root.join(source.uid.as_str());
    let source_refs = gitquarry_mirror_external::git_remote::list_refs_with_sha(&source_path)
        .await
        .unwrap();
    assert_eq!(source_refs.get("refs/heads/main"), Some(&source_sha));

    let (persisted, _) = domain::get(&ctx, &source.uid, &mirror.id).await.unwrap().unwrap();
    assert_eq!(persisted.last_synced.get("refs/heads/main"), None);
    assert_eq!(
        persisted.ref_status.get("refs/heads/main"),
        Some(&access::RefSyncStatus::Diverged)
    );
}

/// The bug the "sync now" button used to have: it called
/// `sync_repo_mirrors`, whose entire job is the periodic tick's "never
/// abort, log-and-continue" resilience — it always returns `Ok(())`
/// regardless of what happened to any mirror, so a real failure (bad
/// url/credential/network) only ever reached stderr, never the click
/// that triggered it. `sync_one_mirror_now` is the fix: same sync, but
/// a real failure comes back as a real `Err` here.
#[tokio::test]
async fn sync_one_mirror_now_surfaces_a_real_failure_instead_of_swallowing_it() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));
    let source = ctx.create_repo("alice/source-broken").await.unwrap().unwrap();
    {
        ctx.open_mirror(&source).await.unwrap();
    }

    allow_mirror_host(&ctx, "example.invalid").await;
    let mirror = domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://example.invalid/acme/target.git".to_string(),
            token: "s3cr3t-token".to_string(),
            enabled: true,
            label: "broken target".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Overridden to a path that genuinely doesn't exist, same
    // in-memory-only override trick the push/pull tests above use —
    // proves this fails for real instead of the assertion just
    // reflecting a bad test setup.
    let mut broken = mirror.clone();
    broken.remote_url = root
        .path()
        .join("nowhere")
        .join("target.git")
        .to_string_lossy()
        .to_string();

    assert!(sync_one_mirror_now(&ctx, &source, &broken).await.is_err());
}

#[test]
fn a_clean_report_has_nothing_to_summarize() {
    let report = SyncReport {
        pushed: 2,
        pulled: 1,
        ..Default::default()
    };
    assert_eq!(report.failure_summary(), None);
}

#[test]
fn a_failed_ref_names_itself_and_counts_the_rest() {
    let one = SyncReport {
        pushed: 1,
        errors: vec![("refs/heads/main".to_string(), "remote rejected".to_string())],
        ..Default::default()
    };
    assert_eq!(one.failure_summary().unwrap(), "refs/heads/main: remote rejected");

    let several = SyncReport {
        errors: vec![
            ("refs/heads/main".to_string(), "remote rejected".to_string()),
            ("refs/heads/dev".to_string(), "remote rejected".to_string()),
            ("refs/tags/v1".to_string(), "remote rejected".to_string()),
        ],
        ..Default::default()
    };
    assert_eq!(
        several.failure_summary().unwrap(),
        "refs/heads/main: remote rejected (and 2 more ref(s) failed)"
    );
}

#[test]
fn a_divergence_is_reported_too_but_never_over_a_real_error() {
    let diverged = SyncReport {
        diverged: vec!["refs/heads/main".to_string()],
        ..Default::default()
    };
    assert!(diverged.failure_summary().unwrap().contains("refs/heads/main"));

    // An outright error is the more actionable of the two, so it wins.
    let both = SyncReport {
        diverged: vec!["refs/heads/main".to_string()],
        errors: vec![("refs/heads/dev".to_string(), "remote rejected".to_string())],
        ..Default::default()
    };
    assert_eq!(both.failure_summary().unwrap(), "refs/heads/dev: remote rejected");
}

/// A pulled ref writes packs and a manifest straight into S3, never
/// through `git receive-pack` — so the repo's own push policy has to be
/// enforced on this path too, or `require_signed_commits` (and protected
/// refs, and the blob ceiling) would be trivially bypassable by pointing a
/// mirror at a repo the puller controls.
#[tokio::test]
async fn sync_mirror_once_refuses_a_pull_that_violates_the_repos_own_policy() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));

    let (_target_dir, target_path, sha) = plain_bare_repo_with_commit("unsigned work\n").await;

    let source = ctx.create_repo("alice/source-policy").await.unwrap().unwrap();
    {
        ctx.open_mirror(&source).await.unwrap();
    }
    let (mut config, etag) = source.remote.get_repo_config().await.unwrap();
    config.require_signed_commits.insert("main".to_string());
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        source.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    allow_mirror_host(&ctx, "example.invalid").await;
    let mut mirror = domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://example.invalid/acme/target.git".to_string(),
            token: "s3cr3t-token".to_string(),
            enabled: true,
            label: "test target".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    mirror.remote_url = target_path.to_string_lossy().to_string();

    let report = sync_mirror_once(&ctx, &source, &mirror).await.unwrap();
    assert_eq!(report.pulled, 0);
    assert!(
        report
            .failure_summary()
            .is_some_and(|msg| msg.contains("signed commits")),
        "expected the signed-commit policy to be the refusal reason, got {:?}",
        report.failure_summary()
    );

    // Neither the local mirror nor S3 may carry the refused ref.
    let source_path = ctx.mirrors_root.join(source.uid.as_str());
    let source_refs = gitquarry_mirror_external::git_remote::list_refs_with_sha(&source_path)
        .await
        .unwrap();
    assert_ne!(source_refs.get("refs/heads/main"), Some(&sha));
    let (manifest, _etag) = source.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/heads/main"), None);
}

/// Somebody deleting a branch is a normal thing to do — it must not leave
/// the mirror permanently red, unlike a genuine divergence.
#[test]
fn a_ref_deleted_on_one_side_isnt_reported_as_a_sweep_failure() {
    let deleted = SyncReport {
        deleted: vec!["refs/heads/gone".to_string()],
        ..Default::default()
    };
    assert_eq!(deleted.failure_summary(), None);

    let diverged = SyncReport {
        diverged: vec!["refs/heads/both-moved".to_string()],
        ..Default::default()
    };
    assert!(diverged.failure_summary().is_some());
}

/// Deleting a repo has to stop its refs flowing out to a third party — the
/// sweep is the only path that can still reach a soft-deleted repo, since
/// ssh/http turn it away long before the pre-serve hook.
#[tokio::test]
async fn a_deleted_repo_stops_being_mirrored_outward() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(root.path()));

    let (_target_dir, target_path, _sha) = plain_bare_repo_with_commit("upstream\n").await;
    let source = ctx.create_repo("alice/source-deleted").await.unwrap().unwrap();
    {
        ctx.open_mirror(&source).await.unwrap();
    }
    allow_mirror_host(&ctx, "example.invalid").await;
    let mut mirror = domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://example.invalid/acme/target.git".to_string(),
            token: "s3cr3t-token".to_string(),
            enabled: true,
            label: "test target".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    mirror.remote_url = target_path.to_string_lossy().to_string();

    let (mut config, etag) = source.remote.get_repo_config().await.unwrap();
    config.deleted_at = Some(1);
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        source.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    sync_repo_mirrors(&ctx, &source).await.unwrap();

    let (persisted, _) = domain::get(&ctx, &source.uid, &mirror.id).await.unwrap().unwrap();
    assert!(persisted.last_synced.is_empty());
    assert!(persisted.ref_status.is_empty());
}
