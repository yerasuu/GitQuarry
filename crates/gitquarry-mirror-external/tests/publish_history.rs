//! History publish runs end to end: real private history pushed into a real
//! repo, rewritten and published into a plain bare repo standing in for the
//! public forge. The cases are the ones that leak, or rewrite somebody's
//! public history, when the rewrite or the epoch rule gets something wrong.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_mirror_external::git_remote;
use gitquarry_mirror_external::publish::audit::{Finding, Gate};
use gitquarry_mirror_external::publish::epoch::EpochBumpRequired;
use gitquarry_mirror_external::publish::filter::{FilterConfig, FilterMode};
use gitquarry_mirror_external::publish::log::{self, PublishRun, RunOutcome};
use gitquarry_mirror_external::publish::preview::preview;
use gitquarry_mirror_external::publish::run::publish_mirror_now;
use gitquarry_mirror_external::publish::{
    self, IdentityMapping, MirrorToken, PublishMirror, PublishMode, UnmappedPolicy,
};
use gitquarry_test_support::{Work, ctx, git};
use std::path::{Path, PathBuf};

const MIT: &str = "SPDX-License-Identifier: MIT\n\nPermission is hereby granted...\n";
const TOKEN: &str = "s3cr3t-history-token";
/// Content that must never reach the public side, in any object.
const PRIVATE: &str = "internal-only: customer acme pays 42\n";

struct Fixture {
    root: tempfile::TempDir,
    _public_dir: tempfile::TempDir,
    ctx: AppContext,
    repo: ResolvedRepo,
    work: Work,
    public: PathBuf,
    mirror: PublishMirror,
}

impl Fixture {
    async fn new() -> Self {
        Self::with(|_| {}).await
    }

    async fn with(customize: impl FnOnce(&mut PublishMirror)) -> Self {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path());
        let repo = ctx.create_repo("alice/private").await.unwrap().unwrap();
        let public_dir = tempfile::tempdir().unwrap();
        let public = bare(public_dir.path()).await;
        allow_host(&ctx, "example.invalid").await;
        let mut wanted = PublishMirror {
            label: "public".to_string(),
            remote_url: "https://example.invalid/acme/public.git".to_string(),
            token: MirrorToken::new(TOKEN.to_string()),
            mode: PublishMode::History,
            filter: FilterConfig {
                mode: FilterMode::Exclude,
                exclude: vec!["deploy/**".to_string()],
                ..Default::default()
            },
            publish_branches: ["main".to_string()].into(),
            publish_tags: ["v*".to_string()].into(),
            unmapped: UnmappedPolicy::Passthrough,
            ..Default::default()
        };
        customize(&mut wanted);
        let mut mirror = publish::create(&ctx, &repo.uid, wanted).await.unwrap();
        // Stored through the validated path, then pointed at the local bare
        // repo in memory only, as the snapshot run's tests do.
        mirror.remote_url = public.to_string_lossy().to_string();
        Self {
            root,
            _public_dir: public_dir,
            ctx,
            repo,
            work: Work::new().await,
            public,
            mirror,
        }
    }

    async fn publish(&self) -> anyhow::Result<PublishRun> {
        self.work.push(&self.repo, "refs/heads/main").await;
        publish_mirror_now(&self.ctx, &self.repo, &self.mirror).await
    }

    async fn public_head(&self) -> Option<String> {
        public_ref(&self.public, "refs/heads/main").await
    }

    async fn log(&self) -> log::PublishLog {
        log::get(&self.ctx, &self.repo.uid, &self.mirror.id).await.unwrap()
    }

    /// Saves `edit` over the stored mirror through the same `update` the
    /// settings page uses, keeping the in-memory remote URL.
    async fn update(&mut self, edit: impl FnOnce(&mut PublishMirror)) -> anyhow::Result<()> {
        let (mut stored, etag) = publish::get(&self.ctx, &self.repo.uid, &self.mirror.id)
            .await
            .unwrap()
            .unwrap();
        edit(&mut stored);
        publish::update(&self.ctx, &self.repo.uid, &stored, &etag).await?;
        stored.remote_url = self.mirror.remote_url.clone();
        self.mirror = stored;
        Ok(())
    }
}

async fn bare(dir: &Path) -> PathBuf {
    let path = dir.join("public.git");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "--quiet", "--bare"]).await;
    path
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

/// Subjects of the public history, oldest first.
async fn public_subjects(public: &Path, rev: &str) -> Vec<String> {
    git(public, &["log", "--reverse", "--format=%s", rev])
        .await
        .lines()
        .map(str::to_string)
        .collect()
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

fn gate_failed(run_error: &anyhow::Error, gate: &str) -> bool {
    let text = run_error.to_string();
    text.contains("blocked by the gates") && text.contains(gate)
}

#[tokio::test]
async fn the_same_history_gives_the_same_public_shas_on_every_run_and_every_node() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.work
        .commit(&[("src/lib.rs", "pub fn run() {}\n")], "add lib\n")
        .await;
    let first = fx.publish().await.unwrap();
    assert_eq!(first.outcome, RunOutcome::Published);
    let head = fx.public_head().await.unwrap();

    // Again, and again after dropping every local copy: the private mirror
    // comes back from the bucket, the publish repo and mapping were only
    // ever in the bucket.
    let again = fx.publish().await.unwrap();
    std::fs::remove_dir_all(fx.root.path().join("mirrors")).unwrap();
    let rebuilt = fx.publish().await.unwrap();
    assert_eq!(again.outcome, RunOutcome::Unchanged);
    assert_eq!(rebuilt.outcome, RunOutcome::Unchanged);
    assert_eq!(fx.public_head().await.as_deref(), Some(head.as_str()));
    // And a run with something new, with no local state left from before:
    // the mapping and the publish repo come back from the bucket. (The
    // unchanged run above never needed the private mirror back.)
    assert!(!fx.root.path().join("mirrors").exists());
    fx.work.commit(&[("src/more.rs", "pub fn more() {}\n")], "more\n").await;
    let more = fx.publish().await.unwrap();
    assert_eq!((more.commits_rewritten, more.commits_reused), (1, 2));
    let next = fx.public_head().await.unwrap();
    assert_eq!(git(&fx.public, &["rev-parse", &format!("{next}^")]).await, head);

    // A second mirror with its own empty mapping and its own remote: what
    // another node, or a restore, computes from scratch.
    let other_dir = tempfile::tempdir().unwrap();
    let other_public = bare(other_dir.path()).await;
    let mut other = publish::create(
        &fx.ctx,
        &fx.repo.uid,
        PublishMirror {
            label: "second".to_string(),
            remote_url: "https://example.invalid/acme/second.git".to_string(),
            ..fx.mirror.clone()
        },
    )
    .await
    .unwrap();
    other.remote_url = other_public.to_string_lossy().to_string();
    publish_mirror_now(&fx.ctx, &fx.repo, &other).await.unwrap();
    assert_eq!(public_ref(&other_public, "refs/heads/main").await, Some(next));
    assert_eq!(
        public_subjects(&other_public, "refs/heads/main").await,
        ["start", "add lib", "more"],
        "every private commit that touches a public path is a public commit"
    );
}

#[tokio::test]
async fn a_publish_after_a_push_rewrites_only_the_new_commits() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.work.commit(&[("src/a.rs", "pub fn a() {}\n")], "a\n").await;
    fx.work.commit(&[("src/b.rs", "pub fn b() {}\n")], "b\n").await;
    let first = fx.publish().await.unwrap();
    assert_eq!((first.commits_rewritten, first.commits_reused), (3, 0));
    let before = fx.public_head().await.unwrap();

    fx.work.commit(&[("src/c.rs", "pub fn c() {}\n")], "c\n").await;
    fx.work.commit(&[("src/d.rs", "pub fn d() {}\n")], "d\n").await;
    let second = fx.publish().await.unwrap();

    assert_eq!(second.outcome, RunOutcome::Published);
    assert_eq!((second.commits_rewritten, second.commits_reused), (2, 3));
    assert!(!second.forced);
    let after = fx.public_head().await.unwrap();
    assert_eq!(git(&fx.public, &["rev-parse", &format!("{after}~2")]).await, before);
    assert_eq!(public_subjects(&fx.public, &after).await, ["start", "a", "b", "c", "d"]);
}

#[tokio::test]
async fn an_excluded_blob_never_exists_on_the_remote_across_history_merges_and_tags() {
    let fx = Fixture::new().await;
    start(&fx).await;
    let path = fx.work.path();
    git(path, &["checkout", "-q", "-b", "side"]).await;
    fx.work
        .commit(
            &[
                ("src/side.rs", "pub fn side() {}\n"),
                ("deploy/side.env", "internal-only: side\n"),
            ],
            "side work\n",
        )
        .await;
    git(path, &["checkout", "-q", "main"]).await;
    fx.work
        .commit(
            &[
                ("src/main.rs", "fn main() { run() }\n"),
                ("deploy/prod.env", "internal-only: renewed\n"),
            ],
            "main work\n",
        )
        .await;
    git(path, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]).await;
    git(path, &["tag", "-a", "-m", "release one", "v1"]).await;
    git(path, &["tag", "v1-light", "HEAD~1"]).await;
    let first_excluded = git(path, &["rev-parse", "HEAD~1~1:deploy/prod.env"]).await;

    fx.publish().await.unwrap();

    let objects = all_objects(&fx.public).await;
    assert!(!objects.contains(&first_excluded), "{objects:?}");
    assert_nowhere(&fx.public, "internal-only").await;
    let listed = git(&fx.public, &["rev-list", "--objects", "--all"]).await;
    assert!(!listed.contains("deploy"), "{listed}");
    let head = fx.public_head().await.unwrap();
    assert_eq!(
        git(&fx.public, &["rev-list", "--parents", "-n", "1", &head])
            .await
            .split(' ')
            .count(),
        3,
        "a merge of two public changes stays a merge"
    );
    assert_eq!(
        git(&fx.public, &["for-each-ref", "--format=%(refname)"]).await,
        "refs/heads/main\nrefs/tags/v1\nrefs/tags/v1-light",
        "side isn't published, v1 and v1-light are"
    );
}

#[tokio::test]
async fn a_secret_moved_under_an_excluded_dir_blocks_the_publish_over_the_whole_history() {
    let fx = Fixture::new().await;
    fx.work
        .commit(&[("LICENSE", MIT), ("prod.env", PRIVATE)], "oops\n")
        .await;
    std::fs::create_dir_all(fx.work.path().join("deploy")).unwrap();
    git(fx.work.path(), &["mv", "prod.env", "deploy/prod.env"]).await;
    fx.work.commit(&[], "move it out of sight\n").await;

    let err = fx.publish().await.unwrap_err();

    assert!(gate_failed(&err, "BlobSet"), "{err:#}");
    assert_eq!(fx.public_head().await, None);
    assert!(all_objects(&fx.public).await.is_empty(), "not even an object was sent");
    let log = fx.log().await;
    let gates = &log.runs[0].refs[0].gates;
    let blob_set = gates.iter().find(|g| g.gate == Gate::BlobSet).unwrap();
    assert!(matches!(
        &blob_set.findings[0],
        Finding::BlobAlsoExcluded { published_path, excluded_path, .. }
            if published_path == "prod.env" && excluded_path == "deploy/prod.env"
    ));
}

#[tokio::test]
async fn content_already_public_and_excluded_later_blocks_the_next_publish() {
    let fx = Fixture::new().await;
    fx.work
        .commit(&[("LICENSE", MIT), ("prod.env", PRIVATE)], "oops\n")
        .await;
    fx.publish().await.unwrap();
    let published = fx.public_head().await.unwrap();
    std::fs::create_dir_all(fx.work.path().join("deploy")).unwrap();
    git(fx.work.path(), &["mv", "prod.env", "deploy/prod.env"]).await;
    fx.work.commit(&[], "move it out of sight\n").await;

    let err = fx.publish().await.unwrap_err();

    assert!(gate_failed(&err, "BlobSet"), "{err:#}");
    assert_eq!(fx.public_head().await.as_deref(), Some(published.as_str()));
}

#[tokio::test]
async fn commits_touching_only_excluded_paths_collapse() {
    let fx = Fixture::new().await;
    fx.work
        .commit(&[("deploy/prod.env", PRIVATE)], "only private, first\n")
        .await;
    start(&fx).await;
    fx.work
        .commit(&[("deploy/prod.env", "internal-only: rotated\n")], "rotate\n")
        .await;
    fx.work
        .commit(&[("src/main.rs", "fn main() { run() }\n")], "change\n")
        .await;

    let run = fx.publish().await.unwrap();

    assert_eq!(run.commits_collapsed, 2);
    let head = fx.public_head().await.unwrap();
    assert_eq!(public_subjects(&fx.public, &head).await, ["start", "change"]);
}

#[tokio::test]
async fn a_merge_whose_parents_collapse_to_one_collapses() {
    let fx = Fixture::new().await;
    start(&fx).await;
    let path = fx.work.path();
    git(path, &["checkout", "-q", "-b", "ops"]).await;
    fx.work
        .commit(&[("deploy/prod.env", "internal-only: ops\n")], "ops only\n")
        .await;
    git(path, &["checkout", "-q", "main"]).await;
    git(path, &["merge", "-q", "--no-ff", "-m", "merge ops", "ops"]).await;

    let run = fx.publish().await.unwrap();

    assert_eq!(run.commits_collapsed, 2, "the ops commit, then the merge");
    let head = fx.public_head().await.unwrap();
    assert_eq!(public_subjects(&fx.public, &head).await, ["start"]);
}

#[tokio::test]
async fn changing_the_filter_without_confirming_the_bump_pushes_nothing() {
    let mut fx = Fixture::new().await;
    start(&fx).await;
    fx.work
        .commit(&[("docs/internal.md", "internal-only: plans\n")], "docs\n")
        .await;
    fx.publish().await.unwrap();
    let head = fx.public_head().await.unwrap();
    let log_before = fx.log().await;

    // The settings edge refuses the edit outright.
    let err = fx
        .update(|m| m.filter.exclude.push("docs/**".to_string()))
        .await
        .unwrap_err();
    assert!(err.downcast_ref::<EpochBumpRequired>().is_some(), "{err:#}");
    assert!(err.to_string().contains("no longer matches"), "{err:#}");
    let stored = publish::get(&fx.ctx, &fx.repo.uid, &fx.mirror.id)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(stored.filter.exclude, ["deploy/**"]);

    // The run edge refuses an output change that never went through it:
    // a mirror changed behind the settings page, and an in-repo filter.
    let mut edited = fx.mirror.clone();
    edited.filter.exclude.push("docs/**".to_string());
    let err = publish_mirror_now(&fx.ctx, &fx.repo, &edited).await.unwrap_err();
    assert!(err.to_string().contains("epoch bump"), "{err:#}");
    fx.work
        .commit(
            &[(".gitquarry/publish.toml", "exclude = [\"docs/**\"]\n")],
            "hide docs\n",
        )
        .await;
    let err = fx.publish().await.unwrap_err();
    assert!(err.to_string().contains("epoch bump"), "{err:#}");

    assert_eq!(fx.public_head().await.as_deref(), Some(head.as_str()));
    let log = fx.log().await;
    assert_eq!(log.epoch, log_before.epoch);
    assert_eq!(log.heads, log_before.heads);
    assert_eq!(log.runs[0].outcome, RunOutcome::Blocked);
}

#[tokio::test]
async fn a_confirmed_bump_force_pushes_with_a_lease_and_records_the_epoch() {
    let mut fx = Fixture::new().await;
    start(&fx).await;
    fx.work
        .commit(&[("docs/internal.md", "internal-only: plans\n")], "docs\n")
        .await;
    fx.work.commit(&[("src/lib.rs", "pub fn run() {}\n")], "lib\n").await;
    fx.publish().await.unwrap();
    let old = fx.public_head().await.unwrap();

    fx.update(|m| {
        m.filter.exclude.push("docs/**".to_string());
        m.filter_epoch += 1;
    })
    .await
    .unwrap();
    let bumped_at = fx.log().await.abandoned.get(&0).copied();
    assert!(bumped_at.is_some(), "the bump records when epoch 0 was left");
    let preview = preview(&fx.ctx, &fx.repo, &fx.mirror).await.unwrap();
    assert_eq!(preview.epoch, 1);
    assert!(preview.force_push);
    let run = fx.publish().await.unwrap();

    assert_eq!(run.outcome, RunOutcome::Published);
    assert!(run.forced);
    assert_eq!(run.epoch, Some(1));
    let new = fx.public_head().await.unwrap();
    assert_eq!(preview.refs[0].public_head.as_deref(), Some(new.as_str()));
    assert_ne!(new, old);
    assert_eq!(public_subjects(&fx.public, &new).await, ["start", "lib"]);
    // The old epoch's objects may stay in the remote unreferenced; what the
    // new history reaches no longer names the newly excluded path.
    let reachable = git(&fx.public, &["rev-list", "--objects", &new]).await;
    assert!(!reachable.contains("docs"), "{reachable}");
    let log = fx.log().await;
    assert_eq!(log.epoch, Some(1));
    assert_eq!(log.runs[0].epoch, Some(1));
    // Epoch 0 stayed protected as the published one until this run, so its
    // grace counts from here, not from the bump.
    assert!(log.abandoned.get(&0).copied() >= bumped_at);
    assert!(!log.abandoned.contains_key(&1));

    // And the next run in the new epoch is a plain fast-forward again.
    fx.work.commit(&[("src/more.rs", "pub fn more() {}\n")], "more\n").await;
    let next = fx.publish().await.unwrap();
    assert!(!next.forced);
    assert_eq!(next.commits_rewritten, 1);
}

#[tokio::test]
async fn a_non_fast_forward_within_an_epoch_is_an_error_and_nothing_is_forced() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.work.commit(&[("src/lib.rs", "pub fn run() {}\n")], "lib\n").await;
    fx.publish().await.unwrap();
    let head = fx.public_head().await.unwrap();

    // The private branch is rewritten: its last commit amended.
    git(fx.work.path(), &["commit", "-q", "--amend", "-m", "lib, reworded"]).await;
    let err = fx.publish().await.unwrap_err();

    assert!(err.to_string().contains("doesn't fast-forward"), "{err:#}");
    assert_eq!(fx.public_head().await.as_deref(), Some(head.as_str()));
    let log = fx.log().await;
    assert_eq!(log.runs[0].outcome, RunOutcome::Failed);
    assert!(!log.runs[0].forced);
}

#[tokio::test]
async fn a_public_ref_moved_by_somebody_else_is_never_forced_even_on_a_new_epoch() {
    let mut fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await.unwrap();
    // A commit added on the public side, on top of what this mirror pushed.
    let other = Work::new().await;
    git(other.path(), &["fetch", "-q", &fx.public.to_string_lossy(), "main"]).await;
    git(other.path(), &["reset", "-q", "--hard", "FETCH_HEAD"]).await;
    let foreign = other.commit(&[("README", "from the forge\n")], "forge edit\n").await;
    git(
        other.path(),
        &["push", "-q", &fx.public.to_string_lossy(), "HEAD:refs/heads/main"],
    )
    .await;

    fx.update(|m| {
        m.filter.exclude.push("LICENSE.bak".to_string());
        m.filter_epoch += 1;
    })
    .await
    .unwrap();
    fx.work
        .commit(&[("src/main.rs", "fn main() { changed() }\n")], "change\n")
        .await;
    let err = fx.publish().await.unwrap_err();

    assert!(err.to_string().contains("didn't publish"), "{err:#}");
    assert_eq!(fx.public_head().await.as_deref(), Some(foreign.as_str()));
    assert_eq!(fx.log().await.runs[0].outcome, RunOutcome::Diverged);
}

#[tokio::test]
async fn a_lease_on_a_value_the_remote_no_longer_holds_refuses_the_whole_push() {
    let dir = tempfile::tempdir().unwrap();
    let public = bare(dir.path()).await;
    let work = Work::new().await;
    let first = work.commit(&[("a", "1\n")], "one\n").await;
    let url = public.to_string_lossy().to_string();
    git(work.path(), &["push", "-q", &url, "HEAD:refs/heads/main"]).await;
    // What a run read before somebody else moved the ref on.
    let moved = work.commit(&[("a", "2\n")], "two\n").await;
    git(work.path(), &["push", "-q", &url, "HEAD:refs/heads/main"]).await;
    git(work.path(), &["checkout", "-q", "--orphan", "rewritten"]).await;
    let replacement = work.commit(&[("b", "3\n")], "three\n").await;

    let err = git_remote::push_atomic_with_leases(
        &work.git_dir(),
        &url,
        TOKEN,
        &[("refs/heads/main".to_string(), first)],
        &[format!("{replacement}:refs/heads/main")],
    )
    .await
    .unwrap_err();

    assert!(!format!("{err:#}").contains(TOKEN));
    assert_eq!(public_ref(&public, "refs/heads/main").await, Some(moved));
}

#[tokio::test]
async fn tags_are_published_pointing_at_the_rewritten_commits() {
    let fx = Fixture::with(|m| {
        m.identities = vec![IdentityMapping {
            private: "test@example.com".to_string(),
            public: "dev@users.noreply.example.org".to_string(),
            name: Some("Dev".to_string()),
        }];
        m.unmapped = UnmappedPolicy::Block;
    })
    .await;
    let private_commit = start(&fx).await;
    let path = fx.work.path();
    git(path, &["tag", "v1-light"]).await;
    // An annotated tag carrying a signature block, written as git stores a
    // signed one: the signature appended to the message.
    let body = format!(
        "object {private_commit}\ntype commit\ntag v1\ntagger Test Author <test@example.com> 1700000000 +0000\n\n\
         release one\n-----BEGIN PGP SIGNATURE-----\n\nnot-a-real-signature\n-----END PGP SIGNATURE-----\n"
    );
    let tag = mktag(path, &body).await;
    git(path, &["update-ref", "refs/tags/v1", &tag]).await;

    let run = fx.publish().await.unwrap();

    assert_eq!(run.outcome, RunOutcome::Published);
    let head = fx.public_head().await.unwrap();
    assert_eq!(public_ref(&fx.public, "refs/tags/v1-light").await, Some(head.clone()));
    let public_tag = public_ref(&fx.public, "refs/tags/v1").await.unwrap();
    assert_ne!(public_tag, tag);
    let published = git(&fx.public, &["cat-file", "tag", &public_tag]).await;
    assert!(
        published.starts_with(&format!("object {head}\ntype commit\ntag v1\n")),
        "{published}"
    );
    assert!(
        published.contains("tagger Dev <dev@users.noreply.example.org> 1700000000 +0000"),
        "{published}"
    );
    assert!(published.ends_with("release one"), "{published}");
    assert!(!published.contains("SIGNATURE"), "{published}");
    assert_nowhere(&fx.public, "test@example.com").await;
}

#[tokio::test]
async fn an_unmapped_tagger_blocks_when_unmapped_identities_block() {
    let fx = Fixture::with(|m| {
        m.identities = vec![IdentityMapping {
            private: "test@example.com".to_string(),
            public: "dev@users.noreply.example.org".to_string(),
            name: None,
        }];
        m.unmapped = UnmappedPolicy::Block;
    })
    .await;
    let private_commit = start(&fx).await;
    let body = format!(
        "object {private_commit}\ntype commit\ntag v1\ntagger Releaser <releaser@internal.corp> 1700000000 +0000\n\n\
         release\n"
    );
    let tag = mktag(fx.work.path(), &body).await;
    git(fx.work.path(), &["update-ref", "refs/tags/v1", &tag]).await;

    let err = fx.publish().await.unwrap_err();

    assert!(gate_failed(&err, "Identity"), "{err:#}");
    assert_eq!(fx.public_head().await, None);
}

async fn mktag(dir: &Path, body: &str) -> String {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("git")
        .current_dir(dir)
        .arg("mktag")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(body.as_bytes()).await.unwrap();
    let out = child.wait_with_output().await.unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[tokio::test]
async fn the_token_never_shows_in_an_error_a_run_record_or_debug_output() {
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
    assert!(!format!("{broken:?}").contains(TOKEN));
}

#[tokio::test]
async fn the_preview_shows_the_epoch_the_counts_and_the_head_a_publish_pushes() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.work
        .commit(&[("deploy/prod.env", "internal-only: rotated\n")], "rotate\n")
        .await;
    fx.work.push(&fx.repo, "refs/heads/main").await;

    let first = preview(&fx.ctx, &fx.repo, &fx.mirror).await.unwrap();
    assert!(first.passed(), "{first:?}");
    assert_eq!(
        (first.epoch, first.force_push, first.epoch_bump_needed),
        (0, false, false)
    );
    let main = &first.refs[0];
    assert_eq!((main.commits_to_rewrite, main.collapsed_commits), (2, 1));
    fx.publish().await.unwrap();
    assert_eq!(main.public_head, fx.public_head().await);
    assert!(all_objects(&fx.public).await.len() > 1);

    fx.work.commit(&[("src/lib.rs", "pub fn run() {}\n")], "lib\n").await;
    fx.work.push(&fx.repo, "refs/heads/main").await;
    let second = preview(&fx.ctx, &fx.repo, &fx.mirror).await.unwrap();
    let main = &second.refs[0];
    assert_eq!((main.commits_to_rewrite, main.commits_reused), (1, 2));
    assert!(!main.unchanged);
    let run = fx.publish().await.unwrap();
    assert_eq!(run.commits_rewritten, 1);
    assert_eq!(main.public_head, fx.public_head().await);
}

/// The measurement behind the subprocess-`git` versus `gix` choice
/// (`docs/publish-mirror-concept.md` §8): a full first publish of a
/// synthetic 10,000-commit history. Ignored by default; run with
/// `cargo test -p gitquarry-mirror-external --release --test publish_history -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn measure_a_full_rewrite_of_ten_thousand_commits() {
    let fx = Fixture::new().await;
    start(&fx).await;
    let mut script = String::new();
    for i in 0..10_000u32 {
        let dir = i % 50;
        let _ = std::fmt::Write::write_fmt(
            &mut script,
            format_args!(
                "commit refs/heads/main\ncommitter Test Author <test@example.com> {} +0000\ndata <<EOF\nchange {i}\nEOF\n\
                 {}M 100644 inline src/m{dir}/f{i}.rs\ndata <<EOF\npub fn f{i}() {{}}\nEOF\nM 100644 inline deploy/d{i}.env\n\
                 data <<EOF\ninternal {i}\nEOF\n\n",
                1_700_000_000 + i,
                if i == 0 { "from refs/heads/main^0\n" } else { "" },
            ),
        );
    }
    {
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new("git")
            .current_dir(fx.work.path())
            .args(["fast-import", "--quiet"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(script.as_bytes()).await.unwrap();
        assert!(child.wait().await.unwrap().success());
    }
    fx.work.push(&fx.repo, "refs/heads/main").await;
    // The private mirror is downloaded before the clock starts.
    preview(
        &fx.ctx,
        &fx.repo,
        &PublishMirror {
            mode: PublishMode::Snapshot,
            ..fx.mirror.clone()
        },
    )
    .await
    .unwrap();

    let started = std::time::Instant::now();
    let run = publish_mirror_now(&fx.ctx, &fx.repo, &fx.mirror).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(run.commits_rewritten, 10_001);
    eprintln!("full history publish of {} commits: {elapsed:?}", run.commits_rewritten);
    let started = std::time::Instant::now();
    fx.work
        .commit(&[("src/after.rs", "pub fn after() {}\n")], "after\n")
        .await;
    let incremental = fx.publish().await.unwrap();
    eprintln!(
        "incremental publish of {} commit(s): {:?}",
        incremental.commits_rewritten,
        started.elapsed()
    );
}
