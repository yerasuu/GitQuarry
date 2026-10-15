//! `gitquarry migrate`: reports (and with `--apply`, runs) the bucket data
//! migrations this binary knows about — see
//! [`gitquarry_core::migrations`] for the mechanism and the list.
//!
//! The server already applies pending migrations itself at startup, so this
//! exists for the cases a startup doesn't cover: seeing what a version bump
//! *would* do before deploying it, and re-running a pass that reported items
//! it couldn't finish without having to restart the server to trigger it.

use anyhow::{Context, Result, bail};
use gitquarry_core::{config, migrations, storage};
use std::sync::Arc;
use tracing::{info, warn};

const USAGE: &str = "\
usage:
  gitquarry migrate [--bucket NAME] [--apply]

Reports which bucket data migrations are still pending and what each one
would change. The server runs these itself on startup; this is for checking
a bucket ahead of a deploy, or re-running a pass that left items behind.

Without --apply nothing is written.";

pub async fn run(rest: &[String]) -> Result<()> {
    let mut bucket_arg: Option<String> = None;
    let mut apply = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => bucket_arg = Some(it.next().cloned().context("--bucket needs a value")?),
            "--apply" => apply = true,
            "--help" | "-h" => {
                println!("{USAGE}"); // stdout, not tracing, because --help output is the CLI's answer
                return Ok(());
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }

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
    let cipher = Arc::new(
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?,
    );

    let stored = migrations::stored_version(backend.as_ref(), &cipher).await?;
    info!(
        "gitquarry: bucket '{bucket}' is at data version {} of {}.",
        stored.version,
        migrations::CURRENT_DATA_VERSION
    );
    let outcomes = if apply {
        migrations::run_pending(&backend, &cipher).await?
    } else {
        migrations::plan(&backend, &cipher).await?
    };
    if outcomes.is_empty() {
        info!("gitquarry: nothing pending.");
        return Ok(());
    }
    for (migration, outcome) in &outcomes {
        let summary = match outcome.summary.as_str() {
            "" => "nothing to change",
            summary => summary,
        };
        info!("gitquarry: {} ({}): {summary}", migration.version, migration.name);
        if outcome.incomplete > 0 {
            warn!(
                "gitquarry: warning: {} item(s) were changed concurrently and left alone -- run again.",
                outcome.incomplete
            );
        }
    }
    if !apply {
        info!("(dry-run -- run again with --apply, or just restart the server, to write the changes)");
    }
    Ok(())
}
