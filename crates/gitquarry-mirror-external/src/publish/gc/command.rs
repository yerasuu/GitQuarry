//! `gitquarry publish-gc`: the operator command around [`super::plan`] and
//! [`super::apply`]. Same shape as `fsck`/`repack`/`restore`: a dry-run
//! unless `--apply`, no interactive prompt, and with `--apply` the epochs
//! about to go are listed as a warning before the first delete.
//!
//! Lives here rather than with the other commands in `gitquarry-domain`
//! because that crate doesn't depend on this one, and the publish mirror
//! entity and run log are only readable from here.

use super::{DEFAULT_GRACE_DAYS, GcReport, MirrorStatus, Verdict};
use anyhow::{Context, Result, bail};
use gitquarry_core::storage::audit::format_unix_secs;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::{config, storage, validate};
use tracing::{info, warn};

const USAGE: &str = "\
usage:
  gitquarry publish-gc [--bucket NAME] [--grace-days N] [--repo UID] [--apply]

Deletes the data of publish-mirror epochs abandoned by an epoch bump (the
old mapping, inverse index, excluded-blob set and publish repo packs under
repos/<uid>/publish/<mirror>/<epoch>/), once they have been abandoned for
longer than the grace period. Without --apply it only lists, per mirror,
what it would delete, with object counts and bytes.

Never touched: a mirror's current epoch and the epoch it last published
(both, when a bump hasn't been published yet), a mirror whose settings or
run log can't be read, and publish data whose mirror no longer exists
(listed only, to be removed by hand). The grace counts from when the epoch
stopped being current and published; an abandoned epoch with no such record
gets one at the first run that sees it -- dry-run included -- so it is never
deleted by that run.

The current epoch's packs (one per publish run) are counted but never
consolidated here.

  --grace-days N  days an abandoned epoch is kept (default 30)
  --repo UID      only this repo (32 lowercase hex digits)
  --apply         delete; deleted epochs can't be rolled back to";

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub bucket: Option<String>,
    pub grace_days: u32,
    /// Already validated: it becomes the `repos/<uid>/publish/` prefix.
    pub repo: Option<Uid>,
    pub apply: bool,
}

/// `Ok(None)` when `--help` was asked for and already printed.
pub fn parse_args(rest: &[String]) -> Result<Option<Args>> {
    let mut args = Args {
        bucket: None,
        grace_days: DEFAULT_GRACE_DAYS,
        repo: None,
        apply: false,
    };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => args.bucket = Some(it.next().cloned().context("--bucket needs a value")?),
            "--grace-days" => {
                let raw = it.next().context("--grace-days needs a number of days")?;
                args.grace_days = raw
                    .parse()
                    .with_context(|| format!("--grace-days: {raw:?} is not a number of days"))?;
            }
            "--repo" => {
                let raw = it.next().context("--repo needs a repo uid")?;
                args.repo = Some(validate::validate_repo_uid(raw)?);
            }
            "--apply" => args.apply = true,
            "--help" | "-h" => {
                println!("{USAGE}"); // stdout, not tracing, because --help output is the CLI's answer
                return Ok(None);
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

pub async fn run(rest: &[String]) -> Result<()> {
    let Some(args) = parse_args(rest)? else {
        return Ok(());
    };
    let config_path = config::Config::resolve_path()?;
    let conf =
        config::Config::load_from(&config_path).with_context(|| format!("couldn't read {}", config_path.display()))?;
    let bucket = match args.bucket {
        Some(b) => b,
        None => conf
            .default_bucket_name()
            .map(str::to_string)
            .context("--bucket NAME required (config has more than one bucket and no default_bucket)")?,
    };
    let bucket_cfg = conf.bucket(&bucket)?.clone();
    let backend = storage::build_backend(&bucket_cfg)?.backend;
    let cipher =
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?;

    let mut report = super::plan(backend.as_ref(), &cipher, args.repo.as_ref(), args.grace_days).await?;
    log_report(&report);
    let removable = report.epochs().filter(|(_, e)| e.verdict == Verdict::Removable).count();
    if removable == 0 {
        info!(target: "publish::gc", bucket = %bucket, "nothing to delete");
        return Ok(());
    }
    if !args.apply {
        info!("(dry-run -- run again with --apply to delete the epochs marked removable)");
        return Ok(());
    }

    warn!(target: "publish::gc", bucket = %bucket, epochs = removable, "DELETING abandoned publish epochs:");
    for (mirror, epoch) in report.epochs().filter(|(_, e)| e.verdict == Verdict::Removable) {
        warn!(
            target: "publish::gc",
            repo = %mirror.repo,
            mirror = %mirror.mirror_id,
            epoch = epoch.epoch,
            objects = epoch.objects,
            bytes = epoch.bytes,
            "  - will delete"
        );
    }
    super::apply(backend.as_ref(), &cipher, &mut report).await;

    let mut failed = 0usize;
    for (mirror, epoch) in report.epochs() {
        match &epoch.verdict {
            Verdict::Deleted => info!(
                target: "publish::gc",
                repo = %mirror.repo,
                mirror = %mirror.mirror_id,
                epoch = epoch.epoch,
                objects = epoch.objects,
                bytes = epoch.bytes,
                "deleted"
            ),
            Verdict::Failed(error) => {
                failed += 1;
                warn!(
                    target: "publish::gc",
                    repo = %mirror.repo,
                    mirror = %mirror.mirror_id,
                    epoch = epoch.epoch,
                    error = %error,
                    "not deleted"
                );
            }
            _ => {}
        }
    }
    if failed > 0 {
        bail!("{failed} epoch(s) failed; run publish-gc again to retry");
    }
    Ok(())
}

fn log_report(report: &GcReport) {
    for mirror in &report.mirrors {
        match &mirror.status {
            MirrorStatus::Live {
                filter_epoch,
                published,
            } => info!(
                target: "publish::gc",
                repo = %mirror.repo,
                mirror = %mirror.mirror_id,
                filter_epoch,
                published = ?published,
                "mirror"
            ),
            MirrorStatus::Unreadable(error) => warn!(
                target: "publish::gc",
                repo = %mirror.repo,
                mirror = %mirror.mirror_id,
                error = %error,
                "mirror can't be read: skipped, nothing of it is touched"
            ),
            MirrorStatus::Orphan => warn!(
                target: "publish::gc",
                repo = %mirror.repo,
                mirror = %mirror.mirror_id,
                "publish data with no mirror: reported only, remove it by hand if it is really unwanted"
            ),
        }
        for epoch in &mirror.epochs {
            let verdict = match &epoch.verdict {
                Verdict::Current => "current, kept".to_string(),
                Verdict::Ahead => "above the mirror's epoch, kept".to_string(),
                Verdict::Untouched => "untouched".to_string(),
                Verdict::InGrace {
                    abandoned_at,
                    eligible_at,
                    recorded_now,
                } => format!(
                    "in grace: abandoned {}{}, removable from {}",
                    format_unix_secs(*abandoned_at),
                    if *recorded_now { " (first seen now)" } else { "" },
                    format_unix_secs(*eligible_at)
                ),
                Verdict::Removable => "REMOVABLE".to_string(),
                Verdict::Deleted => "deleted".to_string(),
                Verdict::Failed(error) => format!("failed: {error}"),
            };
            info!(
                target: "publish::gc",
                repo = %mirror.repo,
                mirror = %mirror.mirror_id,
                epoch = epoch.epoch,
                objects = epoch.objects,
                bytes = epoch.bytes,
                packs = epoch.packs,
                pack_bytes = epoch.pack_bytes,
                "  epoch: {verdict}"
            );
        }
    }
    for key in &report.unrecognized {
        warn!(target: "publish::gc", key = %key, "not a publish epoch object: reported only");
    }
}
