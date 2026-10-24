//! The GC sweep of abandoned publish epochs over real history publishes:
//! what it may delete, what it must never touch, and that the mirror still
//! publishes the same history after a sweep.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::storage::list_all_keys;
use gitquarry_mirror_external::publish::filter::{FilterConfig, FilterMode};
use gitquarry_mirror_external::publish::gc::command::parse_args;
use gitquarry_mirror_external::publish::gc::{self, DEFAULT_GRACE_DAYS, GcReport, MirrorStatus, Verdict};
use gitquarry_mirror_external::publish::log::{PublishLog, RunOutcome};
use gitquarry_mirror_external::publish::run::publish_mirror_now;
use gitquarry_mirror_external::publish::{self, MirrorToken, PublishMirror, PublishMode, UnmappedPolicy};
use gitquarry_test_support::{Work, ctx, git};
use std::path::PathBuf;

const MIT: &str = "SPDX-License-Identifier: MIT\n\nPermission is hereby granted...\n";

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
        let wanted = PublishMirror {
            label: "public".to_string(),
            remote_url: "https://example.invalid/acme/public.git".to_string(),
            token: MirrorToken::new("s3cr3t-gc-token".to_string()),
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
        let mut mirror = publish::create(&ctx, &repo.uid, wanted).await.unwrap();
        // Stored through the validated path, pointed at the local bare repo
        // in memory only, as the history run's own tests do.
        mirror.remote_url = public.to_string_lossy().to_string();
        let fx = Self {
            root,
            _public_dir: public_dir,
            ctx,
            repo,
            work: Work::new().await,
            public,
            mirror,
        };
        fx.work
            .commit(
                &[
                    ("LICENSE", MIT),
                    ("src/main.rs", "fn main() {}\n"),
                    ("docs/internal.md", "plans\n"),
                    ("deploy/prod.env", "internal\n"),
                ],
                "start\n",
            )
            .await;
        fx.work.commit(&[("src/lib.rs", "pub fn run() {}\n")], "lib\n").await;
        fx
    }

    async fn publish(&self) -> RunOutcome {
        self.work.push(&self.repo, "refs/heads/main").await;
        publish_mirror_now(&self.ctx, &self.repo, &self.mirror)
            .await
            .unwrap()
            .outcome
    }

    /// A confirmed bump that excludes `docs/`, through the settings path.
    async fn bump(&mut self) {
        let (mut stored, etag) = publish::get(&self.ctx, &self.repo.uid, &self.mirror.id)
            .await
            .unwrap()
            .unwrap();
        stored.filter.exclude.push("docs/**".to_string());
        stored.filter_epoch += 1;
        publish::update(&self.ctx, &self.repo.uid, &stored, &etag)
            .await
            .unwrap();
        stored.remote_url = self.mirror.remote_url.clone();
        self.mirror = stored;
    }

    /// Epoch 0 published, bumped, epoch 1 published.
    async fn two_epochs() -> Self {
        let mut fx = Self::new().await;
        assert_eq!(fx.publish().await, RunOutcome::Published);
        fx.bump().await;
        assert_eq!(fx.publish().await, RunOutcome::Published);
        fx
    }

    async fn keys(&self, epoch: u32) -> Vec<String> {
        let prefix = format!("repos/{}/publish/{}/{epoch}/", self.repo.uid, self.mirror.id);
        list_all_keys(self.ctx.backend().as_ref(), &prefix).await.unwrap()
    }

    async fn all_publish_keys(&self) -> Vec<String> {
        let prefix = format!("repos/{}/publish/", self.repo.uid);
        list_all_keys(self.ctx.backend().as_ref(), &prefix).await.unwrap()
    }

    async fn plan(&self, grace_days: u32) -> GcReport {
        gc::plan(
            self.ctx.backend().as_ref(),
            &self.ctx.cipher,
            Some(&self.repo.uid),
            grace_days,
        )
        .await
        .unwrap()
    }

    async fn sweep(&self, grace_days: u32) -> GcReport {
        let mut report = self.plan(grace_days).await;
        gc::apply(self.ctx.backend().as_ref(), &self.ctx.cipher, &mut report).await;
        report
    }

    async fn head(&self) -> String {
        git(&self.public, &["rev-parse", "refs/heads/main"]).await
    }

    fn log_collection(&self) -> s3db::Collection {
        s3db::Collection::new_owned(format!("repos/{}/publish-log", self.repo.uid))
    }
}

fn verdicts(report: &GcReport) -> Vec<(u32, Verdict)> {
    report.epochs().map(|(_, e)| (e.epoch, e.verdict.clone())).collect()
}

#[tokio::test]
async fn a_dry_run_lists_the_old_epoch_and_deletes_nothing() {
    let fx = Fixture::two_epochs().await;
    let before = fx.all_publish_keys().await;

    let report = fx.plan(0).await;

    assert_eq!(verdicts(&report), [(0, Verdict::Removable), (1, Verdict::Current)]);
    let (_, old) = report.epochs().next().unwrap();
    assert_eq!(old.objects, fx.keys(0).await.len());
    assert!(old.packs >= 1 && old.bytes > 0 && old.pack_bytes > 0);
    assert_eq!(fx.all_publish_keys().await, before);
}

#[tokio::test]
async fn apply_right_after_a_bump_deletes_nothing_within_the_grace() {
    let fx = Fixture::two_epochs().await;
    let before = fx.all_publish_keys().await;

    let report = fx.sweep(DEFAULT_GRACE_DAYS).await;

    let (_, old) = report.epochs().next().unwrap();
    assert!(
        matches!(old.verdict, Verdict::InGrace { recorded_now: false, eligible_at, abandoned_at }
            if eligible_at == abandoned_at + u64::from(DEFAULT_GRACE_DAYS) * 86_400),
        "{:?}",
        old.verdict
    );
    assert_eq!(fx.all_publish_keys().await, before);
}

#[tokio::test]
async fn past_the_grace_the_old_epoch_goes_and_the_mirror_still_publishes() {
    let fx = Fixture::two_epochs().await;
    let current = fx.keys(1).await;
    let head = fx.head().await;

    let report = fx.sweep(0).await;

    assert_eq!(verdicts(&report), [(0, Verdict::Deleted), (1, Verdict::Current)]);
    assert!(fx.keys(0).await.is_empty());
    assert_eq!(fx.keys(1).await, current);
    let (log, _) = fx
        .log_collection()
        .get::<PublishLog>(fx.ctx.backend().as_ref(), &fx.ctx.cipher, &fx.mirror.id)
        .await
        .unwrap();
    assert!(!log.unwrap().abandoned.contains_key(&0));

    // The same history again, then a new commit with no local state left:
    // the current epoch's mapping and packs are all it needs.
    assert_eq!(fx.publish().await, RunOutcome::Unchanged);
    assert_eq!(fx.head().await, head);
    std::fs::remove_dir_all(fx.root.path().join("mirrors")).unwrap();
    fx.work.commit(&[("src/more.rs", "pub fn more() {}\n")], "more\n").await;
    assert_eq!(fx.publish().await, RunOutcome::Published);
    assert_eq!(git(&fx.public, &["rev-parse", "refs/heads/main^"]).await, head);
    // And a second sweep finds nothing left to do.
    assert_eq!(verdicts(&fx.sweep(0).await), [(1, Verdict::Current)]);
}

#[tokio::test]
async fn the_current_epoch_is_never_removable_even_with_no_grace() {
    let mut fx = Fixture::new().await;
    assert_eq!(fx.publish().await, RunOutcome::Published);
    let before = fx.all_publish_keys().await;
    assert_eq!(verdicts(&fx.sweep(0).await), [(0, Verdict::Current)]);

    // Bumped but not published yet: epoch 0 is still what the public side
    // holds, so it stays current next to the mirror's new epoch.
    fx.bump().await;
    assert_eq!(verdicts(&fx.sweep(0).await), [(0, Verdict::Current)]);
    assert_eq!(fx.all_publish_keys().await, before);
}

#[tokio::test]
async fn an_epoch_with_no_record_is_never_deleted_by_the_sweep_that_finds_it() {
    let fx = Fixture::two_epochs().await;
    // A bucket from before the record existed.
    fx.log_collection()
        .update::<PublishLog, _>(fx.ctx.backend().as_ref(), &fx.ctx.cipher, &fx.mirror.id, |log| {
            log.abandoned.clear();
            Ok(s3db::Update::Write)
        })
        .await
        .unwrap();
    let before = fx.all_publish_keys().await;

    let first = fx.sweep(0).await;

    let (_, old) = first.epochs().next().unwrap();
    assert!(
        matches!(old.verdict, Verdict::InGrace { recorded_now: true, .. }),
        "{:?}",
        old.verdict
    );
    assert_eq!(fx.all_publish_keys().await, before);
    // The record the first sweep left is what the grace counts from.
    assert_eq!(verdicts(&fx.plan(0).await)[0], (0, Verdict::Removable));
    assert!(matches!(
        verdicts(&fx.plan(1).await)[0].1,
        Verdict::InGrace {
            recorded_now: false,
            ..
        }
    ));
}

#[tokio::test]
async fn a_mirror_whose_entity_does_not_open_is_skipped_and_reported() {
    let fx = Fixture::two_epochs().await;
    let backend = fx.ctx.backend();
    let entity = list_all_keys(backend.as_ref(), &format!("repos/{}/publish-mirrors/", fx.repo.uid))
        .await
        .unwrap()
        .into_iter()
        .find(|key| key.contains(&fx.mirror.id))
        .unwrap();
    let (_, etag) = backend.get(&entity).await.unwrap();
    backend
        .put_cas(&entity, b"not a sealed object".to_vec(), etag.as_deref())
        .await
        .unwrap();
    let before = fx.all_publish_keys().await;

    let report = fx.sweep(0).await;

    assert!(matches!(report.mirrors[0].status, MirrorStatus::Unreadable(_)));
    assert_eq!(verdicts(&report), [(0, Verdict::Untouched), (1, Verdict::Untouched)]);
    assert_eq!(fx.all_publish_keys().await, before);
}

#[tokio::test]
async fn the_data_of_a_deleted_mirror_is_only_reported() {
    let fx = Fixture::two_epochs().await;
    publish::delete(&fx.ctx, &fx.repo.uid, &fx.mirror.id).await.unwrap();
    let before = fx.all_publish_keys().await;

    let report = fx.sweep(0).await;

    assert_eq!(report.mirrors[0].status, MirrorStatus::Orphan);
    assert_eq!(verdicts(&report), [(0, Verdict::Untouched), (1, Verdict::Untouched)]);
    assert_eq!(fx.all_publish_keys().await, before);
}

#[tokio::test]
async fn keys_that_are_no_valid_epoch_object_are_reported_and_kept() {
    let fx = Fixture::two_epochs().await;
    let backend = fx.ctx.backend();
    let (uid, id) = (&fx.repo.uid, &fx.mirror.id);
    let odd = [
        format!("repos/{uid}/publish/NOT-A-MIRROR-ID/0/map/00"),
        format!("repos/{uid}/publish/{id}/00/map/00"),
        format!("repos/{uid}/publish/{id}/+0/map/00"),
        format!("repos/{uid}/publish/{id}/0/map/../00"),
        format!("repos/{uid}/publish/{id}/0/notes.txt"),
    ];
    for key in &odd {
        backend.put_cas(key, b"x".to_vec(), None).await.unwrap();
    }

    let report = fx.sweep(0).await;

    let mut reported = report.unrecognized.clone();
    reported.sort();
    let mut expected = odd.to_vec();
    expected.sort();
    assert_eq!(reported, expected);
    assert_eq!(verdicts(&report), [(0, Verdict::Deleted), (1, Verdict::Current)]);
    let left = fx.all_publish_keys().await;
    for key in &odd {
        assert!(left.contains(key), "{key} was deleted");
    }
}

#[test]
fn the_command_validates_its_arguments() {
    let args = |list: &[&str]| parse_args(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>());

    let defaults = args(&[]).unwrap().unwrap();
    assert_eq!(defaults.grace_days, DEFAULT_GRACE_DAYS);
    assert!(!defaults.apply && defaults.repo.is_none());
    let uid = "0123456789abcdef0123456789abcdef";
    let full = args(&["--repo", uid, "--grace-days", "0", "--apply"]).unwrap().unwrap();
    assert_eq!(full.repo.as_ref().map(|u| u.to_string()).as_deref(), Some(uid));
    assert_eq!(full.grace_days, 0);
    assert!(full.apply);

    for bad in [
        &["--repo", "../0123456789abcdef0123456789abcd"][..],
        &["--repo", "0123456789ABCDEF0123456789ABCDEF"],
        &["--repo"],
        &["--grace-days", "-1"],
        &["--grace-days", "thirty"],
        &["--force"],
    ] {
        assert!(args(bad).is_err(), "{bad:?}");
    }
}
