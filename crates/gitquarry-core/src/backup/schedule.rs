//! Running a backup: once now, or forever on a schedule.
//!
//! The schedule keeps no state in memory. Every check re-reads
//! `.conf/global.json` and asks the stored run records when the last backup
//! finished, so enabling a backup (or changing its interval, or its
//! destination) takes effect on the next check without a restart, and a
//! process that dies mid-night doesn't lose track of whether the night's
//! backup already happened.

use crate::app::AppContext;
use crate::backup::dest::{self, Dest, Source};
use crate::backup::inventory::Selection;
use crate::backup::report::{self, BackupRun, Outcome, Trigger};
use crate::backup::{prune, replica, snapshot};
use crate::storage::entities::ServerSettings;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

/// How often a backup runs when nothing says otherwise.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Floor on the configured interval. A backup walks the whole bucket, so a
/// mistyped `"1s"` wouldn't be an aggressive schedule, it would be a
/// permanent listing loop against the storage provider — clamped up rather
/// than rejected, since the intent ("as often as possible") is still served.
pub const MIN_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// How often the loop wakes to *consider* running. Deliberately much shorter
/// than [`DEFAULT_INTERVAL`]: the decision is "has it been long enough",
/// re-derived from settings each time, so a shorter tick is what makes a
/// settings change take effect promptly. Each tick that decides not to run
/// costs one settings read and one small listing.
const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The interval `settings` asks for, clamped to [`MIN_INTERVAL`]. An
/// unparseable value falls back to the default rather than disabling the
/// backup: a typo in a duration must not silently mean "no backups".
pub fn interval(settings: &ServerSettings) -> Duration {
    settings
        .backup_interval
        .as_deref()
        .and_then(|raw| match crate::mirror::gc::parse_age(raw) {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("gitquarry: warning: invalid backup_interval {raw:?} ({e:#}), using the default");
                None
            }
        })
        .unwrap_or(DEFAULT_INTERVAL)
        .max(MIN_INTERVAL)
}

/// Runs one backup now and returns its record.
///
/// `Err` only when the run record itself can't be read or written — an
/// unresolvable destination, a passphrase mismatch, a listing that fails are
/// all recorded *in* the returned run (`Outcome::Failed`, with the reason),
/// because a failed backup is something an operator has to be able to see
/// afterwards, not just something that logged once.
pub async fn run_once(ctx: &AppContext, trigger: Trigger) -> Result<BackupRun> {
    let backend = ctx.backend();
    let source = Source {
        bucket_name: &ctx.bucket,
        backend: &backend,
        cipher: &ctx.cipher,
        config_path: &ctx.config_path,
    };
    run_for(&source, trigger, None, None).await
}

/// [`run_once`] without an [`AppContext`] — what `gitquarry backup` uses,
/// since an operator command has a backend, a cipher and a config path but
/// none of the rest of a running server (mirrors dir, ssh bind, event bus).
///
/// `dest_override` and `selection_override` are the CLI's own flags: a
/// one-off backup somewhere other than the configured destination, without
/// editing the instance's settings to get it.
pub async fn run_for(
    source: &Source<'_>,
    trigger: Trigger,
    dest_override: Option<&str>,
    selection_override: Option<Selection>,
) -> Result<BackupRun> {
    let backend = source.backend;
    let (settings, _etag) = crate::storage::settings::get_settings(backend.as_ref(), source.cipher)
        .await
        .context("reading the instance settings")?;

    let configured = dest_override.unwrap_or(settings.backup_dest.as_deref().unwrap_or_default());
    let mut run = BackupRun::starting(configured, trigger);
    report::save(backend.as_ref(), source.cipher, &run).await?;

    if let Err(e) = execute(source, &settings, dest_override, selection_override, &mut run).await {
        run.fail(format!("{e:#}"));
    } else {
        run.finish();
    }
    report::save(backend.as_ref(), source.cipher, &run).await?;
    // Best-effort: a report collection that didn't get trimmed is untidy,
    // never wrong, and must not turn a successful backup into a failed one.
    if let Err(e) = report::prune_old_runs(backend.as_ref(), source.cipher).await {
        eprintln!("gitquarry: warning: couldn't prune old backup run records: {e:#}");
    }
    Ok(run)
}

/// The run itself. Split out so [`run_for`] has exactly one place to turn an
/// error into a recorded failure.
async fn execute(
    source: &Source<'_>,
    settings: &ServerSettings,
    dest_override: Option<&str>,
    selection_override: Option<Selection>,
    run: &mut BackupRun,
) -> Result<()> {
    let configured = dest_override
        .or(settings.backup_dest.as_deref())
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .context("no backup destination configured (set one at /_settings/backup)")?;
    let target = Dest::parse(configured)?;

    let backend = source.backend;
    let resolved = dest::resolve(&target, source).await?;
    let selection = selection_override.unwrap_or_else(|| Selection::from_settings(settings));
    let concurrency = settings
        .backup_concurrency
        .map(|n| n as usize)
        .unwrap_or(replica::DEFAULT_CONCURRENCY);

    if resolved.replica {
        // Installed (or checked) before anything else is written, and then
        // proven openable with the destination's *own* configured
        // passphrase. `replica::sync` re-checks the config itself, which
        // costs two small reads and keeps that function safe to call on its
        // own; the order here is what matters — a destination filled with
        // data before it has an encryption config can never be given one.
        replica::ensure_same_data_key(backend.as_ref(), resolved.backend.as_ref()).await?;
        resolved.verify_openable().await?;
        replica::sync(backend, &resolved.backend, selection, concurrency, run).await?;
    }

    // The snapshot half: only worth taking when the entity classes are in
    // scope at all, since that's all it captures.
    if settings.backup_snapshot.unwrap_or(true) && selection.entities {
        // The snapshot id is the run's own id: one identifier for both
        // halves, and `report::BackupRun::id`'s shape already satisfies
        // `validate::validate_backup_id`.
        let snapshot_id = run.id.clone();
        let manifest = snapshot::capture(
            backend,
            source.cipher,
            &resolved.backend,
            &resolved.cipher,
            &snapshot_id,
            &snapshot::DEFAULT_CLASSES,
            concurrency,
            run,
        )
        .await?;
        run.snapshot_id = Some(manifest.id);
    }

    // Retention last, and only what's explicitly configured. Every branch
    // here deletes real data at the destination, which is why none of them
    // has a default: an unset field means "keep it", not "keep some
    // reasonable amount".
    if let Some(days) = settings.backup_delete_grace_days.filter(|_| resolved.replica) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let outcome = prune::prune_replica(
            backend,
            &resolved.backend,
            &resolved.cipher,
            u64::from(days) * 86400,
            now,
        )
        .await?;
        if outcome.deleted > 0 || outcome.noted > 0 {
            println!(
                "gitquarry: backup retention: {} deleted, {} newly orphaned, {} back at the source",
                outcome.deleted, outcome.noted, outcome.cleared
            );
        }
    }
    if let Some(keep) = settings.backup_keep {
        prune::prune_snapshots(&resolved.backend, &resolved.cipher, keep).await?;
    }
    if let Some(months) = settings.backup_audit_keep_months.filter(|_| resolved.replica) {
        prune::prune_audit_months(&resolved.backend, months).await?;
    }
    Ok(())
}

/// Checks every [`CHECK_INTERVAL`] whether a backup is due, and runs one when
/// it is. Meant to run detached (`tokio::spawn`), same as
/// `mirror::gc::run_periodic_sweep` and `mirror::deleted_repo_sweep` — a
/// failed backup must never take down ssh://, git-smart-HTTP or the web UI.
pub async fn run_periodic(ctx: Arc<AppContext>) {
    let mut ticker = tokio::time::interval(CHECK_INTERVAL);
    loop {
        ticker.tick().await;
        match due(&ctx).await {
            Ok(false) => continue,
            Ok(true) => match run_once(&ctx, Trigger::Scheduled).await {
                Ok(run) if run.outcome == Outcome::Failed => {
                    eprintln!(
                        "gitquarry: warning: scheduled backup failed: {}",
                        run.error.as_deref().unwrap_or("no reason recorded")
                    );
                }
                Ok(_) => {}
                Err(e) => eprintln!("gitquarry: warning: scheduled backup couldn't be recorded: {e:#}"),
            },
            Err(e) => eprintln!("gitquarry: warning: couldn't tell whether a backup is due: {e:#}"),
        }
    }
}

/// Whether enough time has passed since the last run for another one to be
/// due. Reads the answer out of the bucket rather than remembering it, so a
/// restart doesn't reset the clock and a second instance sharing the bucket
/// doesn't double every backup.
async fn due(ctx: &AppContext) -> Result<bool> {
    let backend = ctx.backend();
    let (settings, _etag) = crate::storage::settings::get_settings(backend.as_ref(), &ctx.cipher).await?;
    if !settings.backup_enabled || settings.backup_dest.as_deref().unwrap_or_default().trim().is_empty() {
        return Ok(false);
    }
    let recent = report::list_recent(backend.as_ref(), &ctx.cipher, 1).await?;
    let Some(last) = recent.first() else {
        // Never backed up: due immediately, which is the useful behavior the
        // first time someone turns this on.
        return Ok(true);
    };
    // A run still marked `Running` is either genuinely in flight or was
    // killed mid-pass. Either way, starting a second one against the same
    // destination would just have the two fight over the same keys — wait
    // for the interval to pass from when it *started* before trying again.
    let since = last.finished_at.unwrap_or(last.started_at);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(now.saturating_sub(since) >= interval(&settings).as_secs())
}
