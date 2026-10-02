//! `gitquarry backup`: runs one backup now, by hand or from cron, and
//! prints what it copied.
//!
//! Same run, same recorded report, same destination rules as the scheduled
//! job (`gitquarry_core::backup::schedule`) — this is a second trigger for
//! it, not a second implementation. An instance that would rather drive
//! backups from the host's own cron than from the server's internal timer
//! just leaves `backup_enabled` off and calls this.

use anyhow::{Context, Result, bail};
use gitquarry_core::backup::dest::Source;
use gitquarry_core::backup::inventory::{Class, Selection};
use gitquarry_core::backup::report::{BackupRun, Outcome, Trigger};
use gitquarry_core::backup::schedule;
use gitquarry_core::{config, storage};
use tracing::{info, warn};

const USAGE: &str = "\
usage:
  gitquarry backup [--bucket NAME] [--dest DEST] [--minimal] [--with-derived]

Copies the whole instance to its configured backup destination: a
byte-for-byte replica at the same keys, plus (unless turned off in
settings) a re-sealed snapshot of the mutable config set.

  --dest DEST      back up somewhere other than the configured destination,
                   without editing settings. One of:
                     bucket:<name>  a [buckets.<name>] section of gitquarry.toml
                     path:/abs/dir  a directory
                     self           this same bucket (snapshot only)
  --minimal        skip the audit trail and the rebuildable caches
  --with-derived   also copy the rebuildable caches (search index,
                   signature cache, backfillable indexes)

Both flags override what settings say for this one run only. The run is
recorded in the bucket either way, and shows up at /_settings/backup.";

/// What the command line asked for. `None` from [`parse_args`] means `--help`
/// was asked for and already printed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub bucket: Option<String>,
    pub dest: Option<String>,
    /// The class selection to use for this run only, `None` to follow
    /// whatever the instance's settings say.
    pub selection: Option<Selection>,
}

pub fn parse_args(rest: &[String]) -> Result<Option<Args>> {
    let mut args = Args::default();
    let mut minimal = false;
    let mut with_derived = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => args.bucket = Some(it.next().cloned().context("--bucket needs a value")?),
            "--dest" => args.dest = Some(it.next().cloned().context("--dest needs a value")?),
            "--minimal" => minimal = true,
            "--with-derived" => with_derived = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    if minimal && with_derived {
        bail!("--minimal and --with-derived contradict each other\n\n{USAGE}");
    }
    args.selection = if minimal {
        Some(Selection::minimal())
    } else if with_derived {
        Some(Selection {
            derived: true,
            ..Selection::default()
        })
    } else {
        None
    };
    Ok(Some(args))
}

pub async fn run(rest: &[String]) -> Result<()> {
    let Some(args) = parse_args(rest)? else {
        return Ok(());
    };
    let Args {
        bucket: bucket_arg,
        dest: dest_override,
        selection,
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

    let source = Source {
        bucket_name: &bucket,
        backend: &backend,
        cipher: &cipher,
        config_path: &config_path,
    };
    let run = schedule::run_for(&source, Trigger::Cli, dest_override.as_deref(), selection).await?;
    print_report(&run);

    // A non-zero exit for anything but a clean run: this is meant to be
    // driven from cron, where the exit code is the only thing that gets
    // noticed.
    match run.outcome {
        Outcome::Ok => Ok(()),
        Outcome::Partial => bail!("{} object(s) failed to copy", run.failures_total),
        Outcome::Failed => bail!("{}", run.error.as_deref().unwrap_or("the backup didn't run")),
        // `run_for` always stamps a final outcome before returning, so this
        // is unreachable in practice — reported rather than assumed away.
        Outcome::Running => bail!("the run never finished recording its outcome"),
    }
}

fn print_report(run: &BackupRun) {
    let totals = run.totals();
    info!("gitquarry: backup {} -> {}", run.id, run.dest);
    if let Some(secs) = run.duration_secs() {
        info!("  took {secs}s");
    }
    for class in Class::REPORTABLE {
        let Some(stat) = run.classes.get(class.label()) else {
            continue;
        };
        info!(
            "  {:<10} copied {:>7}  skipped {:>7}  failed {:>5}  ({} bytes)",
            class.label(),
            stat.copied,
            stat.skipped,
            stat.failed,
            stat.bytes_copied
        );
    }
    info!(
        "  total      copied {:>7}  skipped {:>7}  failed {:>5}",
        totals.copied, totals.skipped, totals.failed
    );
    if let Some(id) = &run.snapshot_id {
        info!("  snapshot   {id}");
    }
    if !run.unknown_keys.is_empty() {
        // Copied regardless — this is the nudge to go classify them in
        // `backup::inventory`, not a warning that anything was lost.
        info!("  unclassified objects copied (worth classifying):");
        for key in &run.unknown_keys {
            info!("    - {key}");
        }
    }
    if !run.failures.is_empty() {
        warn!(
            "  failures ({} total, first {}):",
            run.failures_total,
            run.failures.len()
        );
        for failure in &run.failures {
            warn!("    - {failure}");
        }
    }
}
