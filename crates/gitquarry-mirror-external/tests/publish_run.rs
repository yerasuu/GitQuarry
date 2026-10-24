//! Snapshot publish runs end to end: real private history pushed into a real
//! repo, published into a plain bare repo standing in for the public forge.
//! The cases are the ones that leak, or rewrite somebody's public history,
//! when the run gets its order wrong.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_mirror_external::access::RefSyncStatus;
use gitquarry_mirror_external::publish::filter::{FilterConfig, FilterMode};
use gitquarry_mirror_external::publish::log::{self, RunOutcome};
use gitquarry_mirror_external::publish::run::publish_mirror_now;
use gitquarry_mirror_external::publish::{self, MirrorToken, PublishMirror, Publisher, UnmappedPolicy};
use gitquarry_test_support::{Work, ctx, git};
use std::path::{Path, PathBuf};

const MIT: &str = "SPDX-License-Identifier: MIT\n\nPermission is hereby granted...\n";
const TOKEN: &str = "s3cr3t-publish-token";
/// Content that must never reach the public side, in any object.
const PRIVATE: &str = "internal-only: customer acme pays 42\n";

struct Fixture {
    _root: tempfile::TempDir,
    _public_dir: tempfile::TempDir,
    ctx: AppContext,
    repo: ResolvedRepo,
    work: Work,
    public: PathBuf,
    mirror: PublishMirror,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path());
        let repo = ctx.create_repo("alice/private").await.unwrap().unwrap();
        let public_dir = tempfile::tempdir().unwrap();
        let public = public_dir.path().join("public.git");
        std::fs::create_dir_all(&public).unwrap();
        git(&public, &["init", "--quiet", "--bare"]).await;

        allow_host(&ctx, "example.invalid").await;
        let mut mirror = publish::create(
            &ctx,
            &repo.uid,
            PublishMirror {
                label: "public".to_string(),
                remote_url: "https://example.invalid/acme/public.git".to_string(),
                token: MirrorToken::new(TOKEN.to_string()),
                publisher: Some(Publisher {
                    name: "Acme Publisher".to_string(),
                    email: "publish@acme.example".to_string(),
                }),
                filter: FilterConfig {
                    mode: FilterMode::Exclude,
                    exclude: vec!["deploy/**".to_string()],
                    ..Default::default()
                },
                publish_branches: ["main".to_string()].into(),
                unmapped: UnmappedPolicy::Passthrough,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // Stored through the validated path, then pointed at the local bare
        // repo in memory only, the same way the external mirror's engine
        // tests reach a real push target without a network.
        mirror.remote_url = public.to_string_lossy().to_string();
        Self {
            _root: root,
            _public_dir: public_dir,
            ctx,
            repo,
            work: Work::new().await,
            public,
            mirror,
        }
    }

    async fn publish(&self) -> anyhow::Result<log::PublishRun> {
        self.work.push(&self.repo, "refs/heads/main").await;
        publish_mirror_now(&self.ctx, &self.repo, &self.mirror).await
    }

    async fn public_head(&self) -> Option<String> {
        public_ref(&self.public, "refs/heads/main").await
    }

    async fn log(&self) -> log::PublishLog {
        log::get(&self.ctx, &self.repo.uid, &self.mirror.id).await.unwrap()
    }
}

async fn allow_host(ctx: &AppContext, host: &str) {
    let (mut settings, etag) = gitquarry_core::storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher)
        .await
        .unwrap();
    settings.mirror_extra_hosts.push(host.to_string());
    gitquarry_core::storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher)
        .await
        .unwrap();
}

async fn public_ref(public: &Path, name: &str) -> Option<String> {
    let out = git(public, &["for-each-ref", "--format=%(objectname)", name]).await;
    (!out.is_empty()).then_some(out)
}

/// Every object id in `repo`, reachable or not.
async fn all_objects(repo: &Path) -> Vec<String> {
    git(
        repo,
        &["cat-file", "--batch-all-objects", "--batch-check=%(objectname)"],
    )
    .await
    .lines()
    .map(str::to_string)
    .collect()
}

/// Fails if any object of `repo`, of any type, contains `needle`.
async fn assert_nowhere(repo: &Path, needle: &str) {
    for oid in all_objects(repo).await {
        let kind = git(repo, &["cat-file", "-t", &oid]).await;
        let body = git(repo, &["cat-file", &kind, &oid]).await;
        assert!(!body.contains(needle), "{kind} {oid} carries private content: {body}");
    }
}

async fn start(fx: &Fixture) -> String {
    fx.work
        .commit(
            &[
                ("LICENSE", MIT),
                ("src/main.rs", "fn main() {}\n"),
                ("deploy/prod.env", PRIVATE),
            ],
            "start\n",
        )
        .await
}

#[tokio::test]
async fn a_failing_gate_leaves_the_remote_untouched() {
    let fx = Fixture::new().await;
    fx.work
        .commit(
            &[("src/main.rs", "fn main() {}\n"), ("deploy/prod.env", PRIVATE)],
            "no licence\n",
        )
        .await;

    let err = fx.publish().await.unwrap_err();

    assert!(err.to_string().contains("blocked by the gates"), "{err:#}");
    assert!(err.to_string().contains("License"), "{err:#}");
    assert_eq!(fx.public_head().await, None);
    assert!(all_objects(&fx.public).await.is_empty(), "not even an object was sent");
    let log = fx.log().await;
    assert_eq!(log.runs[0].outcome, RunOutcome::Blocked);
    assert!(log.heads.is_empty() && log.pending.is_empty());
    assert!(matches!(
        log.ref_status.get("refs/heads/main"),
        Some(RefSyncStatus::Error(_))
    ));
}

#[tokio::test]
async fn an_excluded_blob_never_exists_in_the_remote_across_publishes() {
    let fx = Fixture::new().await;
    start(&fx).await;
    // Refs that must never be a source or a target, whatever they hold.
    let head = git(fx.work.path(), &["rev-parse", "HEAD"]).await;
    git(fx.work.path(), &["update-ref", "refs/merge-requests/1/head", &head]).await;
    git(fx.work.path(), &["update-ref", "refs/notes/commits", &head]).await;
    git(fx.work.path(), &["branch", "fix/acme-corp-outage", &head]).await;
    let first = fx.publish().await.unwrap();
    assert_eq!(first.outcome, RunOutcome::Published);

    fx.work
        .commit(
            &[
                ("src/main.rs", "fn main() { run() }\n"),
                ("deploy/prod.env", "internal-only: acme renewed\n"),
            ],
            "second\n",
        )
        .await;
    fx.publish().await.unwrap();

    let refs = git(&fx.public, &["for-each-ref", "--format=%(refname)"]).await;
    assert_eq!(refs, "refs/heads/main");
    let excluded = git(fx.work.path(), &["rev-parse", "HEAD~1:deploy/prod.env"]).await;
    let objects = all_objects(&fx.public).await;
    assert!(!objects.contains(&excluded), "{objects:?}");
    assert_nowhere(&fx.public, "internal-only").await;
    assert_nowhere(&fx.public, "deploy").await;
    let published = git(&fx.public, &["rev-list", "--objects", "refs/heads/main"]).await;
    assert!(!published.contains("deploy"), "{published}");
    // No private commit id and no private message on the public side.
    assert_nowhere(&fx.public, &head).await;
    assert_nowhere(&fx.public, "start").await;
}

#[tokio::test]
async fn an_unchanged_tree_publishes_nothing_the_second_time() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await.unwrap();
    let published = fx.public_head().await.unwrap();
    let objects = all_objects(&fx.public).await;

    let again = fx.publish().await.unwrap();
    // A private commit that only touches excluded paths changes nothing
    // public either.
    fx.work
        .commit(&[("deploy/prod.env", "internal-only: rotated\n")], "rotate\n")
        .await;
    let after_private_only = fx.publish().await.unwrap();

    assert_eq!(again.outcome, RunOutcome::Unchanged);
    assert_eq!(after_private_only.outcome, RunOutcome::Unchanged);
    assert_eq!(fx.public_head().await.as_deref(), Some(published.as_str()));
    assert_eq!(all_objects(&fx.public).await.len(), objects.len());
    let log = fx.log().await;
    assert_eq!(log.runs.len(), 3);
    assert_eq!(log.runs[0].refs[0].public_head.as_deref(), Some(published.as_str()));
    assert_eq!(log.ref_status.get("refs/heads/main"), Some(&RefSyncStatus::InSync));
}

#[tokio::test]
async fn a_changed_tree_publishes_one_fast_forward_child() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await.unwrap();
    let first = fx.public_head().await.unwrap();

    fx.work
        .commit(&[("src/lib.rs", "pub fn run() {}\n")], "add lib\n")
        .await;
    fx.work
        .commit(&[("src/main.rs", "fn main() { run() }\n")], "use lib\n")
        .await;
    let run = fx.publish().await.unwrap();

    let second = fx.public_head().await.unwrap();
    assert_eq!(run.outcome, RunOutcome::Published);
    assert_eq!(run.refs[0].previous_public_head.as_deref(), Some(first.as_str()));
    assert_eq!(run.refs[0].public_head.as_deref(), Some(second.as_str()));
    assert_eq!(git(&fx.public, &["rev-parse", &format!("{second}^")]).await, first);
    assert_eq!(
        git(&fx.public, &["rev-list", "--count", &second]).await,
        "2",
        "one commit per publish, however many private commits"
    );
    let files = git(&fx.public, &["ls-tree", "-r", "--name-only", &second]).await;
    assert_eq!(files, "LICENSE\nsrc/lib.rs\nsrc/main.rs");
    let author = git(&fx.public, &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%s", &second]).await;
    assert_eq!(
        author,
        "Acme Publisher <publish@acme.example>|Acme Publisher <publish@acme.example>|Publish snapshot of main"
    );
    assert_eq!(run.refs[0].published_files, 3);
    assert_eq!(run.refs[0].excluded_entries, 1);
}

#[tokio::test]
async fn a_public_branch_moved_by_somebody_else_is_an_error_and_never_forced() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await.unwrap();
    // An out-of-band rewrite of the public branch, onto unrelated history.
    let other = Work::new().await;
    let foreign = other.commit(&[("README", "somebody else\n")], "elsewhere\n").await;
    git(
        other.path(),
        &[
            "push",
            "-q",
            "--force",
            &fx.public.to_string_lossy(),
            "HEAD:refs/heads/main",
        ],
    )
    .await;
    git(&fx.public, &["config", "receive.denyNonFastForwards", "true"]).await;

    fx.work
        .commit(&[("src/main.rs", "fn main() { changed() }\n")], "change\n")
        .await;
    let err = fx.publish().await.unwrap_err();

    assert!(err.to_string().contains("didn't publish"), "{err:#}");
    assert_eq!(fx.public_head().await.as_deref(), Some(foreign.as_str()));
    let log = fx.log().await;
    assert_eq!(log.runs[0].outcome, RunOutcome::Diverged);
    assert_eq!(log.ref_status.get("refs/heads/main"), Some(&RefSyncStatus::Diverged));
}

#[tokio::test]
async fn a_first_publish_into_a_branch_with_history_is_refused() {
    let fx = Fixture::new().await;
    let other = Work::new().await;
    let existing = other.commit(&[("README", "created by the forge\n")], "init\n").await;
    git(
        other.path(),
        &["push", "-q", &fx.public.to_string_lossy(), "HEAD:refs/heads/main"],
    )
    .await;
    start(&fx).await;

    let err = fx.publish().await.unwrap_err();

    assert!(err.to_string().contains("didn't publish"), "{err:#}");
    assert_eq!(fx.public_head().await.as_deref(), Some(existing.as_str()));
}

#[tokio::test]
async fn a_remote_that_rejects_the_push_is_an_error_and_nothing_is_recorded_as_published() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await.unwrap();
    let first = fx.public_head().await.unwrap();
    let hook = fx.public.join("hooks").join("pre-receive");
    std::fs::write(&hook, "#!/bin/sh\necho 'rejected by policy' >&2\nexit 1\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fx.work
        .commit(&[("src/main.rs", "fn main() { changed() }\n")], "change\n")
        .await;
    let err = fx.publish().await.unwrap_err();

    assert!(err.to_string().contains("rejected"), "{err:#}");
    assert_eq!(fx.public_head().await.as_deref(), Some(first.as_str()));
    let log = fx.log().await;
    assert_eq!(log.runs[0].outcome, RunOutcome::Failed);
    assert_eq!(
        log.heads["refs/heads/main"].commit, first,
        "the head the next run builds on"
    );

    std::fs::remove_file(&hook).unwrap();
    let retried = fx.publish().await.unwrap();
    assert_eq!(retried.outcome, RunOutcome::Published);
    let second = fx.public_head().await.unwrap();
    assert_eq!(git(&fx.public, &["rev-parse", &format!("{second}^")]).await, first);
}

#[tokio::test]
async fn the_token_never_shows_in_an_error_or_a_run_record() {
    let fx = Fixture::new().await;
    start(&fx).await;
    let mut broken = fx.mirror.clone();
    broken.remote_url = fx.public.with_file_name("nowhere.git").to_string_lossy().to_string();
    fx.work.push(&fx.repo, "refs/heads/main").await;

    let err = publish_mirror_now(&fx.ctx, &fx.repo, &broken).await.unwrap_err();

    assert!(!format!("{err:#} {err:?}").contains(TOKEN), "{err:#}");
    let log = fx.log().await;
    assert!(log.last_error.is_some());
    assert!(!format!("{log:?}").contains(TOKEN));
    assert!(!format!("{broken:?}").contains(TOKEN), "Debug redacts the token");
    let stored = publish::get(&fx.ctx, &fx.repo.uid, &fx.mirror.id)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(stored.token.expose(), TOKEN, "stored for the push, sealed by s3db");
}

#[tokio::test]
async fn a_mirror_without_a_publisher_is_blocked_before_any_push() {
    let fx = Fixture::new().await;
    start(&fx).await;
    let mut anonymous = fx.mirror.clone();
    anonymous.publisher = None;
    fx.work.push(&fx.repo, "refs/heads/main").await;

    let err = publish_mirror_now(&fx.ctx, &fx.repo, &anonymous).await.unwrap_err();

    assert!(err.to_string().contains("Identity"), "{err:#}");
    assert_eq!(fx.public_head().await, None);
}
