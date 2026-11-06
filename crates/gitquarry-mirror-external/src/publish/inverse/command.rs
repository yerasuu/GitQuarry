//! `gitquarry publish-inverse-backfill`: the operator command around
//! [`super::backfill`]. Same shape as `publish-gc`: a dry-run unless
//! `--apply`, no interactive prompt, and here for the same reason (only
//! this crate reads publish mirror data).

use super::backfill;
use crate::publish::gc::keys;
use anyhow::{Context, Result, bail};
use gitquarry_core::storage::REPOS_PREFIX;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::{config, storage, validate};
use tracing::{info, warn};

const USAGE: &str = "\
usage:
  gitquarry publish-inverse-backfill [--bucket NAME] [--repo UID] [--apply]

Rebuilds the inverse index (public commit -> private commits, under
repos/<uid>/publish/<mirror>/<epoch>/inverse/) of every publish-mirror epoch
from that epoch's own mapping, for epochs published before the index
existed. Without --apply it only counts, per epoch, the entries missing.

Only adds: entries already there are kept, and an epoch whose index is
complete is not rewritten, so running it again does nothing extra. Never
reads the private repo and never deletes.

  --repo UID  only this repo (32 lowercase hex digits)
  --apply     write the missing entries";

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub bucket: Option<String>,
    /// Already validated: it becomes the `repos/<uid>/publish/` prefix.
    pub repo: Option<Uid>,
    pub apply: bool,
}

/// `Ok(None)` when `--help` was asked for and already printed.
pub fn parse_args(rest: &[String]) -> Result<Option<Args>> {
    let mut args = Args {
        bucket: None,
        repo: None,
        apply: false,
    };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => args.bucket = Some(it.next().cloned().context("--bucket needs a value")?),
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

    let prefix = match &args.repo {
        Some(uid) => format!("{REPOS_PREFIX}/{uid}/publish/"),
        None => format!("{REPOS_PREFIX}/"),
    };
    let listing = keys::classify(backend.list_prefix(&prefix).await?);
    let (mut missing, mut failed) = (0usize, 0usize);
    for ((uid, mirror_id), epochs) in &listing.mirrors {
        for epoch in epochs
            .iter()
            .filter(|(_, keys)| !keys.map.is_empty())
            .map(|(epoch, _)| *epoch)
        {
            let planned = match backfill::plan(backend.as_ref(), &cipher, uid, mirror_id, epoch).await {
                Ok(planned) => planned,
                Err(error) => {
                    failed += 1;
                    warn!(
                        target: "publish::inverse",
                        repo = %uid,
                        mirror = %mirror_id,
                        epoch,
                        error = %format!("{error:#}"),
                        "can't be planned"
                    );
                    continue;
                }
            };
            info!(
                target: "publish::inverse",
                repo = %uid,
                mirror = %mirror_id,
                epoch,
                mapped = planned.mapped(),
                missing = planned.missing(),
                "epoch"
            );
            if planned.missing() == 0 {
                continue;
            }
            missing += planned.missing();
            if !args.apply {
                continue;
            }
            match backfill::apply(backend.as_ref(), &cipher, &planned).await {
                Ok(()) => info!(
                    target: "publish::inverse",
                    repo = %uid,
                    mirror = %mirror_id,
                    epoch,
                    added = planned.missing(),
                    "backfilled"
                ),
                Err(error) => {
                    failed += 1;
                    warn!(
                        target: "publish::inverse",
                        repo = %uid,
                        mirror = %mirror_id,
                        epoch,
                        error = %format!("{error:#}"),
                        "not backfilled"
                    );
                }
            }
        }
    }
    if missing == 0 && failed == 0 {
        info!(target: "publish::inverse", bucket = %bucket, "every inverse index is complete");
    } else if !args.apply && missing > 0 {
        info!(
            target: "publish::inverse",
            missing,
            "(dry-run -- run again with --apply to write the missing entries)"
        );
    }
    if failed > 0 {
        bail!("{failed} epoch(s) failed; run publish-inverse-backfill again to retry");
    }
    Ok(())
}
