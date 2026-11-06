//! The inbound dry run over real publish runs: a private history published
//! into a plain bare repo standing in for the public forge, pull requests
//! opened on it by a contributor clone under `refs/pull/<n>/head`, and the
//! verdict for each of the cases of `docs/publish-mirror-inbound-concept.md`
//! §3. Every dry run is checked to leave the bucket and the private repo
//! exactly as it found them.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::storage::list_all_keys;
use gitquarry_mirror_external::access::ExternalMirrorProvider;
use gitquarry_mirror_external::publish::filter::{FilterConfig, FilterMode};
use gitquarry_mirror_external::publish::inbound::command::{parse_args, provider_for, render};
use gitquarry_mirror_external::publish::inbound::{
    Cap, DryRun, MAX_CHANGED_FILES, Reason, Refusal, Request, Translation, Verdict, dry_run,
};
use gitquarry_mirror_external::publish::log::RunOutcome;
use gitquarry_mirror_external::publish::run::publish_mirror_now;
use gitquarry_mirror_external::publish::{
    self, IdentityMapping, MirrorToken, PublishMirror, PublishMode, UnmappedPolicy,
};
use gitquarry_test_support::{Work, ctx, git};
use std::path::{Path, PathBuf};

const MIT: &str = "SPDX-License-Identifier: MIT\n\nPermission is hereby granted...\n";

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
        Self::with(|_| {}).await
    }

    async fn with(customize: impl FnOnce(&mut PublishMirror)) -> Self {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path());
        let repo = ctx.create_repo("alice/private").await.unwrap().unwrap();
        let public_dir = tempfile::tempdir().unwrap();
        let public = public_dir.path().join("public.git");
        std::fs::create_dir_all(&public).unwrap();
        git(&public, &["init", "--quiet", "--bare"]).await;
        let (mut settings, etag) = gitquarry_core::storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher)
            .await
            .unwrap();
        settings.mirror_extra_hosts.push("example.invalid".to_string());
        gitquarry_core::storage::settings::put_settings(
            ctx.backend().as_ref(),
            &settings,
            etag.as_deref(),
            &ctx.cipher,
        )
        .await
        .unwrap();
        let mut wanted = PublishMirror {
            label: "public".to_string(),
            remote_url: "https://example.invalid/acme/public.git".to_string(),
            token: MirrorToken::new("push-token".to_string()),
            inbound_token: MirrorToken::new("read-token".to_string()),
            mode: PublishMode::History,
            filter: FilterConfig {
                mode: FilterMode::Exclude,
                exclude: vec!["deploy/**".to_string()],
                ..Default::default()
            },
            publish_branches: ["main".to_string()].into(),
            unmapped: UnmappedPolicy::Passthrough,
            ..Default::default()
        };
        customize(&mut wanted);
        let mut mirror = publish::create(&ctx, &repo.uid, wanted).await.unwrap();
        // Stored through the validated path, then pointed at the local bare
        // repo in memory only, as the publish run tests do.
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

    async fn publish(&self) {
        self.work.push(&self.repo, "refs/heads/main").await;
        let run = publish_mirror_now(&self.ctx, &self.repo, &self.mirror).await.unwrap();
        assert_eq!(run.outcome, RunOutcome::Published);
    }

    async fn bump_epoch(&mut self, exclude: &str) {
        let (mut stored, etag) = publish::get(&self.ctx, &self.repo.uid, &self.mirror.id)
            .await
            .unwrap()
            .unwrap();
        stored.filter.exclude.push(exclude.to_string());
        stored.filter_epoch += 1;
        publish::update(&self.ctx, &self.repo.uid, &stored, &etag)
            .await
            .unwrap();
        stored.remote_url = self.mirror.remote_url.clone();
        self.mirror = stored;
    }

    async fn public_main(&self) -> String {
        git(&self.public, &["rev-parse", "refs/heads/main"]).await
    }

    /// A contributor's clone of the public repo, on a branch at `start`.
    async fn contributor(&self, start: &str) -> Contributor {
        let dir = tempfile::tempdir().unwrap();
        git(
            dir.path(),
            &["clone", "--quiet", &self.public.to_string_lossy(), "clone"],
        )
        .await;
        let contributor = Contributor { dir };
        git(&contributor.path(), &["checkout", "--quiet", "-B", "pr", start]).await;
        contributor
    }

    /// The dry run of pull request `pr` against `main`, with the bucket and
    /// the private repo compared before and after.
    async fn dry_run(&self, pr: &str) -> DryRun {
        let local = self.ctx.open_mirror_read(&self.repo).await.unwrap();
        let bucket = self.bucket().await;
        let private = private_state(&local.path).await;
        let request = Request {
            pr: pr.parse().unwrap(),
            base: "main".parse().unwrap(),
            provider: ExternalMirrorProvider::GitHub,
        };
        let out = dry_run(
            self.ctx.backend().as_ref(),
            &self.ctx.cipher,
            &self.repo.uid,
            &local.path,
            &self.mirror,
            &request,
            &self.mirror.inbound_token,
        )
        .await
        .unwrap();
        assert!(self.bucket().await == bucket, "the dry run wrote to the bucket");
        assert_eq!(
            private_state(&local.path).await,
            private,
            "the dry run wrote to the private repo"
        );
        out
    }

    /// Every key of the bucket with its stored bytes.
    async fn bucket(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for key in list_all_keys(self.ctx.backend().as_ref(), "").await.unwrap() {
            let (body, _etag) = self.ctx.backend().get(&key).await.unwrap();
            out.push((key, body));
        }
        out.sort();
        out
    }
}

struct Contributor {
    dir: tempfile::TempDir,
}

impl Contributor {
    fn path(&self) -> PathBuf {
        self.dir.path().join("clone")
    }

    async fn commit(&self, files: &[(&str, &str)], message: &str) -> String {
        for (path, content) in files {
            let file = self.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, content).unwrap();
        }
        self.commit_all(message).await
    }

    async fn commit_all(&self, message: &str) -> String {
        git(&self.path(), &["add", "-A"]).await;
        git(&self.path(), &["commit", "--quiet", "-m", message]).await;
        git(&self.path(), &["rev-parse", "HEAD"]).await
    }

    async fn open(&self, pr: u32) {
        let refspec = format!("HEAD:refs/pull/{pr}/head");
        git(&self.path(), &["push", "--quiet", "origin", &refspec]).await;
    }
}

async fn private_state(repo: &Path) -> (String, String) {
    (
        git(repo, &["for-each-ref"]).await,
        git(
            repo,
            &["cat-file", "--batch-all-objects", "--batch-check=%(objectname)"],
        )
        .await,
    )
}

async fn start(fx: &Fixture) -> String {
    fx.work
        .commit(
            &[
                ("LICENSE", MIT),
                ("src/main.rs", "fn main() {}\n"),
                ("deploy/prod.env", "internal: keep out\n"),
            ],
            "start\n",
        )
        .await
}

fn translated(out: &DryRun) -> &Translation {
    match &out.verdict {
        Verdict::Translatable(translation) => translation,
        Verdict::Refused(refusal) => panic!("refused: {refusal:?}"),
    }
}

fn refused(out: &DryRun) -> &Refusal {
    match &out.verdict {
        Verdict::Refused(refusal) => refusal,
        Verdict::Translatable(translation) => panic!("translated: {translation:?}"),
    }
}

/// Cherry-picks `public` onto the private `onto` in the private work repo,
/// keeping its committer date: what a maintainer would get by hand, and so
/// an independent account of the commit the overlay must compute.
async fn cherry_pick(work: &Work, public_repo: &Path, onto: &str, public: &[String]) -> Vec<String> {
    git(
        work.path(),
        &["fetch", "--quiet", &public_repo.to_string_lossy(), "refs/pull/1/head"],
    )
    .await;
    git(work.path(), &["checkout", "--quiet", "--detach", onto]).await;
    let mut picked = Vec::new();
    for commit in public {
        let date = git(work.path(), &["log", "-1", "--format=%cd", "--date=raw", commit]).await;
        let out = tokio::process::Command::new("git")
            .current_dir(work.path())
            .args(["-c", "commit.gpgsign=false", "cherry-pick", "--allow-empty", commit])
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .await
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        picked.push(git(work.path(), &["rev-parse", "HEAD"]).await);
    }
    picked
}

#[tokio::test]
async fn a_clean_pull_request_translates_onto_the_private_tree_and_round_trips() {
    let fx = Fixture::new().await;
    let start = start(&fx).await;
    fx.publish().await;
    let base = fx.public_main().await;
    let pr = fx.contributor(&base).await;
    let one = pr
        .commit(&[("src/main.rs", "fn main() { run() }\n")], "Call run\n")
        .await;
    let two = pr.commit(&[("src/run.rs", "pub fn run() {}\n")], "Add run\n").await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    assert_eq!(out.epoch, 0);
    assert_eq!(out.head.as_deref(), Some(two.as_str()));
    assert_eq!(out.base.as_deref(), Some(base.as_str()));
    let translation = translated(&out);
    assert_eq!(translation.private_base, start);
    assert_eq!(translation.collapsed_onto_base, 0);
    let public: Vec<String> = translation.commits.iter().map(|c| c.public.clone()).collect();
    assert_eq!(public, [one, two]);
    // The overlay keeps the excluded file the public side never saw, so the
    // translation is exactly the cherry-pick onto the private commit.
    let private: Vec<String> = translation.commits.iter().map(|c| c.private.clone()).collect();
    assert_eq!(cherry_pick(&fx.work, &fx.public, &start, &public).await, private);
    assert!(render(&out).contains("verdict: translatable"));
}

#[tokio::test]
async fn a_base_other_private_commits_collapsed_onto_resolves_to_the_one_that_produced_it() {
    let fx = Fixture::new().await;
    let start = start(&fx).await;
    fx.work
        .commit(&[("deploy/prod.env", "internal: rotated\n")], "rotate\n")
        .await;
    fx.publish().await;
    let pr = fx.contributor(&fx.public_main().await).await;
    pr.commit(&[("src/main.rs", "fn main() { run() }\n")], "Call run\n")
        .await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    let translation = translated(&out);
    assert_eq!(translation.private_base, start);
    assert_eq!(translation.collapsed_onto_base, 1);
    assert_eq!(translation.commits.len(), 1);
}

#[tokio::test]
async fn a_change_at_an_excluded_path_is_refused_without_naming_the_pattern() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await;
    let pr = fx.contributor(&fx.public_main().await).await;
    let commit = pr.commit(&[("deploy/new.env", "token=1\n")], "Add config\n").await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    let refusal = refused(&out);
    assert_eq!(refusal.reason, Reason::ExcludedPath);
    assert_eq!(refusal.path.as_deref(), Some("deploy/new.env"));
    assert_eq!(refusal.public_commit.as_deref(), Some(commit.as_str()));
    assert!(!render(&out).contains("deploy/**"));
}

#[tokio::test]
async fn sensitive_paths_are_refused_even_where_the_filter_publishes() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await;
    let base = fx.public_main().await;
    for (n, path) in [
        (1, ".github/workflows/ci.yml"),
        (2, "src/.gitattributes"),
        (3, ".gitmodules"),
    ] {
        let pr = fx.contributor(&base).await;
        pr.commit(&[(path, "x\n")], "Sneak\n").await;
        pr.open(n).await;
        let out = fx.dry_run(&n.to_string()).await;
        assert_eq!(refused(&out).reason, Reason::SensitivePath, "{path}");
    }
}

#[tokio::test]
async fn a_merge_commit_in_the_pull_request_is_refused() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await;
    let base = fx.public_main().await;
    let pr = fx.contributor(&base).await;
    pr.commit(&[("src/a.rs", "pub fn a() {}\n")], "a\n").await;
    git(&pr.path(), &["checkout", "--quiet", "-b", "side", &base]).await;
    pr.commit(&[("src/b.rs", "pub fn b() {}\n")], "b\n").await;
    git(&pr.path(), &["checkout", "--quiet", "pr"]).await;
    git(
        &pr.path(),
        &["merge", "--quiet", "--no-ff", "-m", "Merge side\n", "side"],
    )
    .await;
    let merge = git(&pr.path(), &["rev-parse", "HEAD"]).await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    let refusal = refused(&out);
    assert_eq!(refusal.reason, Reason::MergeCommit);
    assert_eq!(refusal.public_commit.as_deref(), Some(merge.as_str()));
}

#[tokio::test]
async fn a_symlink_is_refused() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await;
    let pr = fx.contributor(&fx.public_main().await).await;
    std::os::unix::fs::symlink("../../etc/passwd", pr.path().join("src/link")).unwrap();
    pr.commit_all("Add a link\n").await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    let refusal = refused(&out);
    assert_eq!(refusal.reason, Reason::Symlink);
    assert_eq!(refusal.path.as_deref(), Some("src/link"));
}

/// Somebody pushed straight to the public branch (the forge's merge button,
/// a force push): the pull request sits on a commit the mirror never made,
/// and the commits in between are not translated as if they were its own.
#[tokio::test]
async fn a_pull_request_on_a_foreign_public_commit_has_no_private_base() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await;
    let outsider = fx.contributor(&fx.public_main().await).await;
    let foreign = outsider.commit(&[("src/x.rs", "pub fn x() {}\n")], "x\n").await;
    git(
        &outsider.path(),
        &["push", "--quiet", "--force", "origin", "HEAD:refs/heads/main"],
    )
    .await;
    let pr = fx.contributor(&foreign).await;
    pr.commit(&[("src/y.rs", "pub fn y() {}\n")], "y\n").await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    let refusal = refused(&out);
    assert_eq!(refusal.reason, Reason::NoPrivateBase);
    assert_eq!(refusal.public_commit.as_deref(), Some(foreign.as_str()));
}

#[tokio::test]
async fn a_pull_request_on_an_earlier_epoch_is_refused_with_that_epoch() {
    let mut fx = Fixture::new().await;
    fx.work
        .commit(
            &[
                ("LICENSE", MIT),
                ("src/main.rs", "fn main() {}\n"),
                ("docs/guide.md", "# Guide\n"),
            ],
            "start\n",
        )
        .await;
    fx.publish().await;
    let pr = fx.contributor(&fx.public_main().await).await;
    pr.commit(&[("src/main.rs", "fn main() { run() }\n")], "Call run\n")
        .await;
    pr.open(1).await;
    fx.bump_epoch("docs/**").await;
    fx.publish().await;

    let out = fx.dry_run("1").await;

    assert_eq!(out.epoch, 1);
    assert_eq!(refused(&out).reason, Reason::OldBaseEpoch(0));
}

#[tokio::test]
async fn a_pull_request_over_the_changed_file_cap_is_refused_whole() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await;
    let pr = fx.contributor(&fx.public_main().await).await;
    for i in 0..=MAX_CHANGED_FILES {
        let file = pr.path().join(format!("gen/{}/{i}.txt", i % 50));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, format!("{i}\n")).unwrap();
    }
    pr.commit_all("Generate\n").await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    assert_eq!(refused(&out).reason, Reason::TooLarge(Cap::ChangedFiles));
}

/// A contributor commits under an address the identity map rewrites (an
/// internal one, used by mistake): the translation would publish under the
/// mapped address, a commit the pull request doesn't have.
#[tokio::test]
async fn a_translation_that_publishes_differently_fails_the_round_trip() {
    let fx = Fixture::with(|m| {
        m.identities = vec![IdentityMapping {
            private: "test@example.com".to_string(),
            public: "dev@users.noreply.example.org".to_string(),
            name: None,
        }];
    })
    .await;
    start(&fx).await;
    fx.publish().await;
    let pr = fx.contributor(&fx.public_main().await).await;
    let commit = pr
        .commit(&[("src/main.rs", "fn main() { run() }\n")], "Call run\n")
        .await;
    pr.open(1).await;

    let out = fx.dry_run("1").await;

    let refusal = refused(&out);
    assert_eq!(refusal.reason, Reason::RoundTripMismatch);
    assert_eq!(refusal.public_commit.as_deref(), Some(commit.as_str()));
}

#[test]
fn the_dry_run_command_takes_validated_arguments_only() {
    let args = |list: &[&str]| parse_args(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let base = [
        "--repo",
        "alice/private",
        "--mirror",
        "0123456789abcdef",
        "--pr",
        "7",
        "--base",
        "main",
    ];

    let parsed = args(&base).unwrap().unwrap();
    assert_eq!(parsed.pr.get(), 7);
    assert_eq!(parsed.base.as_str(), "main");
    assert!(!parsed.push_token && parsed.provider.is_none());
    for (flag, bad) in [
        ("--pr", "0"),
        ("--pr", "-1"),
        ("--pr", "7:refs/heads/x"),
        ("--base", "main:refs/heads/x"),
        ("--base", "-x"),
        ("--base", "a..b"),
        ("--base", "+main"),
        ("--mirror", "../etc"),
        ("--provider", "bitbucket"),
    ] {
        let mut list = base.to_vec();
        list.extend([flag, bad]);
        assert!(args(&list).is_err(), "{flag} {bad} accepted");
    }
    assert!(
        args(&["--pr", "7"]).is_err(),
        "--repo, --mirror and --base are required"
    );
    assert_eq!(
        provider_for("https://gitlab.com/acme/public.git"),
        ExternalMirrorProvider::GitLab
    );
    assert_eq!(
        provider_for("https://git.example.org/acme/public.git"),
        ExternalMirrorProvider::Other
    );
}
