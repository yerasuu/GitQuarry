//! `gitquarry restore`: writes a snapshot back over the live data, or brings
//! back one repo (`--repo <uid>`) from the replica and a snapshot — see
//! `gitquarry_core::backup::restore_repo`.
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
use gitquarry_core::backup::dest::{self, Dest, Resolved, Source};
use gitquarry_core::backup::restore_repo::{self, EntitySource};
use gitquarry_core::backup::snapshot;
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::{config, storage, validate};
use std::sync::Arc;
use tracing::{info, warn};

const USAGE: &str = "\
usage:
  gitquarry restore --list [--bucket NAME] [--from DEST]
  gitquarry restore <snapshot-id> [--bucket NAME] [--from DEST] [--apply]
  gitquarry restore --repo UID [<snapshot-id>] [--bucket NAME] [--from DEST] [--apply]

Writes a snapshot's objects back over the live data, at their original
keys. Dry-run unless --apply is passed.

  --list        list the snapshots available and stop
  --repo UID    restore only this repo (e.g. one purged by mistake): its
                packs, refs, LFS objects and releases from the replica, its
                config and other entities from <snapshot-id> (default: the
                newest snapshot holding it). refs.json is written last.
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
    /// Already validated: it becomes the `repos/<uid>/` key prefix.
    pub repo: Option<Uid>,
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
            "--repo" => {
                let raw = it.next().context("--repo needs a repo uid")?;
                args.repo = Some(validate::validate_repo_uid(raw)?);
            }
            "--apply" => args.apply = true,
            "--help" | "-h" => {
                println!("{USAGE}"); // stdout, not tracing, because --help output is the CLI's answer
                return Ok(None);
            }
            other if other.starts_with('-') => bail!("unexpected argument: {other}\n\n{USAGE}"),
            id if args.snapshot_id.is_none() => args.snapshot_id = Some(id.to_string()),
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    if args.list && args.repo.is_some() {
        bail!("--list and --repo don't go together: list first, then restore\n\n{USAGE}");
    }
    if !args.list && args.snapshot_id.is_none() && args.repo.is_none() {
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
        repo,
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

    if let Some(uid) = repo {
        return run_repo(&resolved, &backend, &cipher, &uid, snapshot_id.as_deref(), apply).await;
    }

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

async fn run_repo(
    resolved: &Resolved,
    live: &Arc<dyn Backend>,
    cipher: &Cipher,
    uid: &Uid,
    snapshot_id: Option<&str>,
    apply: bool,
) -> Result<()> {
    if !resolved.replica {
        bail!(
            "restoring a repo needs the replica for its git data, and a 'self' backup destination \
             holds only snapshots: pass --from with the replica's destination"
        );
    }
    let report = restore_repo::restore_repo(&resolved.backend, live, cipher, uid, snapshot_id, !apply).await?;
    let entities_from = match &report.entities_from {
        EntitySource::Snapshot(id) => format!("snapshot {id}"),
        EntitySource::Replica => "the replica (no snapshot holds this repo)".to_string(),
    };
    if !report.refs_restored && report.failures.is_empty() {
        warn!("the replica holds no refs.json for repo {uid}: it comes back with no branches or tags");
    }
    if report.undeleted {
        info!("the backed-up config marks repo {uid} as deleted; the restore clears that so it serves again");
    }
    if !apply {
        info!(
            "gitquarry: would restore repo {uid}: {} object(s) ({} bytes) from the replica and {} entity object(s) \
             from {entities_from}, refs.json last.",
            report.replica_objects, report.replica_bytes, report.entity_objects
        );
        info!("(dry-run -- run again with --apply to actually overwrite them)");
        return Ok(());
    }

    info!(
        "gitquarry: restored repo {uid}: {} object(s) ({} bytes) from the replica and {} entity object(s) from \
         {entities_from}.",
        report.replica_objects, report.replica_bytes, report.entity_objects
    );
    if !report.failures.is_empty() {
        warn!("failures ({}):", report.failures.len());
        for failure in &report.failures {
            warn!("  - {failure}");
        }
        bail!(
            "{} object(s) failed to restore, so refs.json was not written and the repo isn't usable yet; \
             run the restore again",
            report.failures.len()
        );
    }
    // The repo's name claim and its owner/visible/permission indexes are
    // derived and not part of a per-repo restore: without the backfill the
    // repo exists but nothing resolves its name.
    info!("next: run 'gitquarry backfill-repo-index --apply' so the repo's name and listings point at it again.");
    Ok(())
}
