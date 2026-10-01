//! `gitquarry restore`: writes a snapshot back over the live data.
//!
//! Deliberately CLI-only, with no web equivalent. A restore overwrites live
//! config, users, groups, permissions and repo metadata wholesale — it is
//! not an "undo" button, it's the thing you do once, on purpose, after
//! something went wrong, and it should require reaching the machine.
//!
//! Dry-run by default (same `--apply` shape as `fsck`/`repack`/
//! `backfill-repo-index`): it prints how many objects it would write and
//! from which snapshot, and does nothing until asked twice.
//!
//! Note what a restore is *not* needed for. The replica half of a backup
//! keeps every object at its own key in a bucket carrying the source's own
//! encryption config — so recovering a whole dead instance is
//! `gitquarry --bucket <replica>`, not a restore. This command is for the
//! other case: the instance is fine, but its config or metadata is wrong and
//! there's a snapshot of when it wasn't.

use anyhow::{Context, Result, bail};
use gitquarry_core::backup::dest::{self, Dest, Source};
use gitquarry_core::backup::snapshot;
use gitquarry_core::{config, storage};
use tracing::{info, warn};

const USAGE: &str = "\
usage:
  gitquarry restore --list [--bucket NAME] [--from DEST]
  gitquarry restore <snapshot-id> [--bucket NAME] [--from DEST] [--apply]

Writes a snapshot's objects back over the live data, at their original
keys. Dry-run unless --apply is passed.

  --list        list the snapshots available and stop
  --from DEST   where the snapshots live, if not the configured backup
                destination (bucket:<name> | path:/abs/dir | self)

A whole-instance recovery does NOT need this: the replica half of a backup
is a bootable bucket, so that case is 'gitquarry --bucket <replica>'. This
command is for putting config/metadata back to what a snapshot holds while
the instance itself is fine.

Restoring rewrites live objects. There is no undo beyond taking another
snapshot first.";

/// What the command line asked for. `None` from [`parse_args`] means
/// `--help` was asked for and already printed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub bucket: Option<String>,
    pub from: Option<String>,
    pub snapshot_id: Option<String>,
    pub list: bool,
    /// Dry-run unless this is set — a restore overwrites live data, so it
    /// takes two runs to do anything.
    pub apply: bool,
}

pub fn parse_args(rest: &[String]) -> Result<Option<Args>> {
    let mut args = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => args.bucket = Some(it.next().cloned().context("--bucket needs a value")?),
            "--from" => args.from = Some(it.next().cloned().context("--from needs a value")?),
            "--list" => args.list = true,
            "--apply" => args.apply = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other if other.starts_with('-') => bail!("unexpected argument: {other}\n\n{USAGE}"),
            id if args.snapshot_id.is_none() => args.snapshot_id = Some(id.to_string()),
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    if !args.list && args.snapshot_id.is_none() {
        bail!("which snapshot? pass an id, or --list to see them\n\n{USAGE}");
    }
    Ok(Some(args))
}

pub async fn run(rest: &[String]) -> Result<()> {
    let Some(args) = parse_args(rest)? else {
        return Ok(());
    };
    let Args {
        bucket: bucket_arg,
        from,
        snapshot_id,
        list,
        apply,
    } = args;

    let config_path = config::Config::resolve_path()?;
    let conf =
        config::Config::load_from(&config_path).with_context(|| format!("couldn't read {}", config_path.display()))?;
    let bucket = match bucket_arg {
        Some(b) => b,
        None => conf
            .default_bucket_name()
            .map(str::to_string)
            .context("--bucket NAME required (config has more than one bucket and no default_bucket)")?,
    };
    let bucket_cfg = conf.bucket(&bucket)?.clone();
    let backend = storage::build_backend(&bucket_cfg)?.backend;
    let cipher = std::sync::Arc::new(
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?,
    );

    // Where the snapshots are: `--from`, else whatever the instance's own
    // settings point at. Read from the bucket rather than re-specified, so
    // the common case needs no flag at all.
    let (settings, _etag) = storage::settings::get_settings(backend.as_ref(), &cipher).await?;
    let configured = from
        .as_deref()
        .or(settings.backup_dest.as_deref())
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .context("no backup destination configured and no --from given: where are the snapshots?")?;
    let target = Dest::parse(configured)?;
    let source = Source {
        bucket_name: &bucket,
        backend: &backend,
        cipher: &cipher,
        config_path: &config_path,
    };
    let resolved = dest::resolve(&target, &source).await?;

    let available = snapshot::list(&resolved.backend, &resolved.cipher).await?;
    if list {
        if available.is_empty() {
            info!("gitquarry: no snapshots at {configured}");
            return Ok(());
        }
        info!("snapshots at {configured} (newest first):");
        for manifest in &available {
            info!(
                "  {}  {} object(s), {} bytes{}",
                manifest.id,
                manifest.objects,
                manifest.bytes,
                if manifest.complete { "" } else { "  (INCOMPLETE)" }
            );
        }
        return Ok(());
    }

    let id = snapshot_id.expect("checked above");
    let report = snapshot::restore(
        &resolved.backend,
        &resolved.cipher,
        backend.as_ref(),
        &cipher,
        &id,
        !apply,
    )
    .await?;

    if !report.manifest.complete {
        // Restoring a partial snapshot is allowed — it's still the best copy
        // available — but it must never look like a full one.
        warn!("warning: snapshot {id} is INCOMPLETE (some objects failed to capture)");
    }
    if !apply {
        info!(
            "gitquarry: would restore {} object(s) ({} bytes) from snapshot {id} over the live data in {bucket}.",
            report.restored, report.bytes
        );
        info!("(dry-run -- run again with --apply to actually overwrite them)");
        return Ok(());
    }

    info!(
        "gitquarry: restored {} object(s) ({} bytes) from snapshot {id}.",
        report.restored, report.bytes
    );
    if !report.failures.is_empty() {
        warn!("failures ({}):", report.failures.len());
        for failure in &report.failures {
            warn!("  - {failure}");
        }
    }
    // The derived indexes aren't in a snapshot (see `backup::inventory`),
    // and a restored entity set can disagree with whatever they currently
    // hold — saying so here is the difference between a working restore and
    // a repo that exists but shows up nowhere.
    info!("next: run 'gitquarry backfill-repo-index --apply' to rebuild the derived indexes.");
    if !report.failures.is_empty() {
        bail!("{} object(s) failed to restore", report.failures.len());
    }
    Ok(())
}
