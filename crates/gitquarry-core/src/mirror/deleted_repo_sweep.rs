//! Purges soft-deleted repos (`domain::repo::delete`) once their grace
//! window (`domain::repo::DELETE_GRACE_SECS`) elapses — the automatic half
//! of "delete has a recovery window": nothing prompts for a second, final
//! confirmation, a repo nobody restored in time just quietly becomes gone.
//!
//! Scans `.conf/index/repo-deleted` rather than every repo in the bucket —
//! same reasoning [`crate::storage::settings::list_visible_repo_uids`]
//! already has for the anonymous repo listing.

use crate::app::{AppContext, ResolvedRepo};
use crate::domain;
use crate::storage;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

/// Much coarser than [`super::gc::run_periodic_sweep`]'s interval:
/// purging a repo is a rare, deliberate-consequence event, not a cache
/// trim, so there's no reason to check more often than this.
const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Runs [`sweep_once`] forever on [`SWEEP_INTERVAL`]. Meant to run detached
/// (`tokio::spawn`), same as [`super::gc::run_periodic_sweep`] and
/// `gitquarry_mirror_external::engine::run_periodic_sync` — a sweep failure must never take
/// down ssh://, git-smart-HTTP, or the web UI.
pub async fn run_periodic_sweep(ctx: Arc<AppContext>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(e) = sweep_once(&ctx).await {
            warn!(error = %e, "deleted-repo sweep failed");
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub async fn sweep_once(ctx: &AppContext) -> Result<()> {
    sweep_once_impl(ctx).await
}
#[cfg(not(any(test, feature = "test-util")))]
async fn sweep_once(ctx: &AppContext) -> Result<()> {
    sweep_once_impl(ctx).await
}

async fn sweep_once_impl(ctx: &AppContext) -> Result<()> {
    let backend = ctx.backend();
    for uid in storage::settings::list_deleted_repo_uids(backend.as_ref()).await? {
        let resolved = ResolvedRepo {
            remote: ctx.remote_for_uid(&uid),
            uid: uid.clone(),
        };
        let (config, _etag) = match resolved.remote.get_repo_config().await {
            Ok(c) => c,
            Err(e) => {
                warn!(%uid, error = %e, "deleted-repo sweep: couldn't read config");
                continue;
            }
        };
        let Some(deleted_at) = config.deleted_at else {
            // Index says deleted, config disagrees (restored since, or a
            // stale entry left by a failed index write) -- self-heal the
            // index and move on, same tolerance every other index in this
            // codebase already has.
            storage::settings::set_deleted_index(backend.as_ref(), &ctx.cipher, &uid, false).await;
            continue;
        };
        if domain::repo::grace_seconds_remaining(deleted_at) > 0 {
            continue;
        }
        if let Err(e) = domain::repo::purge(ctx, &resolved, &config.full_name).await {
            warn!(%uid, error = %e, "deleted-repo sweep: couldn't purge");
        }
    }
    Ok(())
}
