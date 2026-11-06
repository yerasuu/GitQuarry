//! The inverse index a history publish keeps per epoch (public commit →
//! the private commits behind it), read back through
//! [`publish::inverse::private_commits`] after real publish runs into a
//! plain bare repo standing in for the public forge.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::crypto::CompressionStrategy;
use gitquarry_core::storage::{Cas, list_all_keys};
use gitquarry_mirror_external::publish::filter::{FilterConfig, FilterMode};
use gitquarry_mirror_external::publish::inverse::backfill::{self, Backfill};
use gitquarry_mirror_external::publish::inverse::command::parse_args;
use gitquarry_mirror_external::publish::inverse::private_commits;
use gitquarry_mirror_external::publish::log::{PublishRun, RunOutcome};
use gitquarry_mirror_external::publish::run::publish_mirror_now;
use gitquarry_mirror_external::publish::{
    self, IdentityMapping, MirrorToken, PublishMirror, PublishMode, UnmappedPolicy,
};
use gitquarry_test_support::{Work, ctx, git};
use std::path::PathBuf;

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
            token: MirrorToken::new("token".to_string()),
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

    async fn publish(&self) -> anyhow::Result<PublishRun> {
        self.work.push(&self.repo, "refs/heads/main").await;
        publish_mirror_now(&self.ctx, &self.repo, &self.mirror).await
    }

    async fn public_rev(&self, rev: &str) -> String {
        git(&self.public, &["rev-parse", rev]).await
    }

    async fn inverse(&self, public: &str) -> anyhow::Result<Vec<String>> {
        private_commits(
            &self.ctx,
            &self.repo.uid,
            &self.mirror.id,
            self.mirror.filter_epoch,
            public,
        )
        .await
    }

    async fn plan_backfill(&self) -> anyhow::Result<Backfill> {
        backfill::plan(
            self.ctx.backend().as_ref(),
            &self.ctx.cipher,
            &self.repo.uid,
            &self.mirror.id,
            self.mirror.filter_epoch,
        )
        .await
    }

    fn table(&self, name: &str) -> String {
        format!(
            "repos/{}/publish/{}/{}/{name}/",
            self.repo.uid, self.mirror.id, self.mirror.filter_epoch
        )
    }

    /// Every object under the epoch's `name` table, as stored (sealed).
    async fn raw(&self, name: &str) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for key in list_all_keys(self.ctx.backend().as_ref(), &self.table(name))
            .await
            .unwrap()
        {
            let (body, _etag) = self.ctx.backend().get(&key).await.unwrap();
            out.push((key, body));
        }
        out.sort();
        out
    }

    /// Replaces the inverse shard holding `public` with `plain`, sealed as
    /// the server would seal it unless `sealed` is false.
    async fn overwrite_inverse_shard(&self, public: &str, plain: impl FnOnce(Vec<u8>) -> Vec<u8>, sealed: bool) {
        let key = format!("{}{}", self.table("inverse"), &public[..2]);
        let (body, etag) = self.ctx.backend().get(&key).await.unwrap();
        let opened = self.ctx.cipher.open(&key, &body).unwrap();
        let replaced = plain(opened);
        let body = if sealed {
            self.ctx
                .cipher
                .seal(&key, &replaced, CompressionStrategy::None)
                .unwrap()
        } else {
            replaced
        };
        let written = self.ctx.backend().put_cas(&key, body, etag.as_deref()).await.unwrap();
        assert!(matches!(written, Cas::Ok));
    }
}

fn sorted(mut shas: Vec<String>) -> Vec<String> {
    shas.sort();
    shas
}

async fn start(fx: &Fixture) -> String {
    fx.work
        .commit(&[("LICENSE", MIT), ("src/main.rs", "fn main() {}\n")], "start\n")
        .await
}

async fn excluded_only(fx: &Fixture, content: &str) -> String {
    fx.work.commit(&[("deploy/prod.env", content)], "rotate\n").await
}

#[tokio::test]
async fn every_public_commit_resolves_to_the_private_commits_behind_it() {
    let fx = Fixture::new().await;
    let first = excluded_only(&fx, "internal: first\n").await;
    let start = start(&fx).await;
    let rotate = excluded_only(&fx, "internal: rotated\n").await;
    let change = fx
        .work
        .commit(&[("src/main.rs", "fn main() { run() }\n")], "change\n")
        .await;

    let run = fx.publish().await.unwrap();

    assert_eq!(run.outcome, RunOutcome::Published);
    let public_change = fx.public_rev("refs/heads/main").await;
    let public_start = fx.public_rev("refs/heads/main^").await;
    assert_eq!(fx.inverse(&public_change).await.unwrap(), [change]);
    assert_eq!(
        fx.inverse(&public_start).await.unwrap(),
        sorted(vec![start, rotate]),
        "the commit that only touched excluded paths collapsed onto its parent's public commit"
    );
    // The first commit collapsed away entirely: no public commit names it.
    for public in [&public_change, &public_start] {
        assert!(!fx.inverse(public).await.unwrap().contains(&first));
    }
    assert!(
        fx.inverse(&first).await.unwrap().is_empty(),
        "a private sha is no public key"
    );
    assert!(fx.inverse("not-a-sha").await.is_err());
}

#[tokio::test]
async fn a_second_run_adds_only_the_new_private_commits() {
    let fx = Fixture::new().await;
    let start = start(&fx).await;
    let a = fx.work.commit(&[("src/a.rs", "pub fn a() {}\n")], "a\n").await;
    fx.publish().await.unwrap();
    let public_start = fx.public_rev("refs/heads/main^").await;
    let public_a = fx.public_rev("refs/heads/main").await;

    let rotate = excluded_only(&fx, "internal: rotated\n").await;
    let b = fx.work.commit(&[("src/b.rs", "pub fn b() {}\n")], "b\n").await;
    let run = fx.publish().await.unwrap();

    assert_eq!((run.commits_rewritten, run.commits_reused), (2, 2));
    let public_b = fx.public_rev("refs/heads/main").await;
    assert_eq!(fx.public_rev("refs/heads/main^").await, public_a);
    assert_eq!(fx.inverse(&public_start).await.unwrap(), [start]);
    assert_eq!(fx.inverse(&public_a).await.unwrap(), sorted(vec![a, rotate]));
    assert_eq!(fx.inverse(&public_b).await.unwrap(), [b]);
}

#[tokio::test]
async fn storing_the_same_history_again_leaves_the_inverse_untouched() {
    let fx = Fixture::new().await;
    start(&fx).await;
    excluded_only(&fx, "internal: rotated\n").await;
    fx.work.commit(&[("src/a.rs", "pub fn a() {}\n")], "a\n").await;
    let first = fx.publish().await.unwrap();
    let head = fx.public_rev("refs/heads/main").await;
    let before = fx.raw("inverse").await;
    assert!(!before.is_empty());

    // Without the forward map and the public ref, the next run rewrites the
    // whole history from the root and stores every inverse entry again.
    for (key, _) in fx.raw("map").await {
        fx.ctx.backend().delete(&key).await.unwrap();
    }
    git(&fx.public, &["update-ref", "-d", "refs/heads/main"]).await;
    let again = fx.publish().await.unwrap();

    assert_eq!(again.outcome, RunOutcome::Published);
    assert_eq!(again.commits_rewritten, first.commits_rewritten);
    assert_eq!(fx.public_rev("refs/heads/main").await, head);
    // Sealing draws a fresh nonce, so any rewrite of a shard, even with the
    // same entries, would change its bytes.
    assert_eq!(fx.raw("inverse").await, before);
}

#[tokio::test]
async fn a_corrupt_inverse_shard_is_a_hard_error() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await.unwrap();
    let head = fx.public_rev("refs/heads/main").await;

    fx.overwrite_inverse_shard(
        &head,
        |mut plain| {
            plain.pop();
            plain
        },
        true,
    )
    .await;
    let err = fx.inverse(&head).await.unwrap_err();
    assert!(format!("{err:#}").contains("not a valid inverse shard"), "{err:#}");
    // A run that adds to that shard refuses it too, rather than replacing it.
    excluded_only(&fx, "internal: rotated\n").await;
    let err = fx.publish().await.unwrap_err();
    assert!(format!("{err:#}").contains("not a valid inverse shard"), "{err:#}");

    fx.overwrite_inverse_shard(&head, |_| b"GQI1 plaintext, not sealed".to_vec(), false)
        .await;
    let err = fx.inverse(&head).await.unwrap_err();
    assert!(format!("{err:#}").contains("doesn't open"), "{err:#}");
}

/// Two nodes publish the same history at once under different identity
/// maps, both from an empty mapping: the second to store finds every
/// private commit already mapped elsewhere and stops before its push. The
/// inverse accepting many private commits per key doesn't loosen this.
#[tokio::test]
async fn a_private_commit_mapped_to_two_public_commits_in_one_epoch_is_a_hard_error() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.work.commit(&[("src/a.rs", "pub fn a() {}\n")], "a\n").await;
    fx.work.push(&fx.repo, "refs/heads/main").await;
    let other_root = tempfile::tempdir().unwrap();
    let other_node = AppContext {
        backend: fx.ctx.backend(),
        ..ctx(other_root.path())
    };
    let renamed = PublishMirror {
        identities: vec![IdentityMapping {
            private: "test@example.com".to_string(),
            public: "dev@users.noreply.example.org".to_string(),
            name: None,
        }],
        ..fx.mirror.clone()
    };

    let (one, two) = tokio::join!(
        publish_mirror_now(&fx.ctx, &fx.repo, &fx.mirror),
        publish_mirror_now(&other_node, &fx.repo, &renamed),
    );

    let (won, lost) = match (one, two) {
        (Ok(won), Err(lost)) | (Err(lost), Ok(won)) => (won, lost),
        (one, two) => panic!("expected exactly one run to fail: {one:?} / {two:?}"),
    };
    assert_eq!(won.outcome, RunOutcome::Published);
    assert!(
        format!("{lost:#}").contains("already mapped to something else in this epoch"),
        "{lost:#}"
    );
    assert_eq!(
        Some(fx.public_rev("refs/heads/main").await),
        won.refs[0].public_head,
        "the public side holds the winner's history only"
    );
}

/// An epoch published before the index existed, stood in for by one whose
/// inverse shards were deleted: the backfill brings back what the runs
/// wrote, collapsed commits included, from the forward map alone.
#[tokio::test]
async fn a_backfill_rebuilds_a_missing_inverse_from_the_mapping() {
    let fx = Fixture::new().await;
    let start = start(&fx).await;
    fx.publish().await.unwrap();
    let rotate = excluded_only(&fx, "internal: rotated\n").await;
    let a = fx.work.commit(&[("src/a.rs", "pub fn a() {}\n")], "a\n").await;
    fx.publish().await.unwrap();
    let public_start = fx.public_rev("refs/heads/main^").await;
    let public_a = fx.public_rev("refs/heads/main").await;
    let written = fx.plan_backfill().await.unwrap();
    assert_eq!((written.mapped(), written.missing()), (3, 0));

    for (key, _) in fx.raw("inverse").await {
        fx.ctx.backend().delete(&key).await.unwrap();
    }
    assert!(fx.inverse(&public_start).await.unwrap().is_empty());
    let planned = fx.plan_backfill().await.unwrap();
    assert_eq!((planned.mapped(), planned.missing()), (3, 3));
    assert!(fx.raw("inverse").await.is_empty(), "planning writes nothing");

    backfill::apply(fx.ctx.backend().as_ref(), &fx.ctx.cipher, &planned)
        .await
        .unwrap();
    assert_eq!(fx.inverse(&public_start).await.unwrap(), sorted(vec![start, rotate]));
    assert_eq!(fx.inverse(&public_a).await.unwrap(), [a]);
    assert_eq!(fx.plan_backfill().await.unwrap().missing(), 0);

    // The index the next run adds to is the backfilled one, and the public
    // history it publishes is unchanged by it.
    let b = fx.work.commit(&[("src/b.rs", "pub fn b() {}\n")], "b\n").await;
    fx.publish().await.unwrap();
    assert_eq!(fx.public_rev("refs/heads/main^").await, public_a);
    assert_eq!(fx.inverse(&fx.public_rev("refs/heads/main").await).await.unwrap(), [b]);
}

#[tokio::test]
async fn a_backfill_over_a_complete_inverse_rewrites_nothing() {
    let fx = Fixture::new().await;
    start(&fx).await;
    excluded_only(&fx, "internal: rotated\n").await;
    fx.publish().await.unwrap();
    let before = fx.raw("inverse").await;

    let planned = fx.plan_backfill().await.unwrap();
    assert_eq!(planned.missing(), 0);
    backfill::apply(fx.ctx.backend().as_ref(), &fx.ctx.cipher, &planned)
        .await
        .unwrap();

    // A resealed shard would differ: sealing draws a fresh nonce.
    assert_eq!(fx.raw("inverse").await, before);
}

#[tokio::test]
async fn a_backfill_refuses_a_corrupt_inverse_shard() {
    let fx = Fixture::new().await;
    start(&fx).await;
    fx.publish().await.unwrap();
    let head = fx.public_rev("refs/heads/main").await;
    fx.overwrite_inverse_shard(
        &head,
        |mut plain| {
            plain.pop();
            plain
        },
        true,
    )
    .await;

    let err = fx.plan_backfill().await.unwrap_err();
    assert!(format!("{err:#}").contains("not a valid inverse shard"), "{err:#}");
}

#[tokio::test]
async fn an_epoch_with_no_mapping_plans_nothing() {
    let fx = Fixture::new().await;
    let planned = fx.plan_backfill().await.unwrap();
    assert_eq!((planned.mapped(), planned.missing()), (0, 0));
}

#[test]
fn the_backfill_command_is_a_dry_run_unless_asked_to_apply() {
    let args = |list: &[&str]| parse_args(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>());

    let defaults = args(&[]).unwrap().unwrap();
    assert!(!defaults.apply && defaults.repo.is_none() && defaults.bucket.is_none());
    let uid = "0123456789abcdef0123456789abcdef";
    let full = args(&["--repo", uid, "--bucket", "b", "--apply"]).unwrap().unwrap();
    assert_eq!(full.repo.as_ref().map(|u| u.to_string()).as_deref(), Some(uid));
    assert!(full.apply);
    assert!(args(&["--repo", "../etc"]).is_err());
    assert!(args(&["--force"]).is_err());
}
