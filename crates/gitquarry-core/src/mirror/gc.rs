//! Local mirror cache eviction.
//!
//! A mirror under `mirrors_root` is disposable: it's always re-derivable
//! from S3 (see [`crate::app::AppContext::open_mirror`]), so it's trimmed
//! like a cache instead of left to grow forever. Two independent limits:
//! - age: a mirror nobody has queried in `mirror_max_age` gets removed.
//! - size: if the mirrors dir is still over `mirror_max_size_bytes` after
//!   age-based eviction, the least-recently-queried survivors get removed
//!   (oldest first) until it's back under the cap.

use crate::app::AppContext;
use anyhow::{Context, Result, bail};
use futures::stream::{self, StreamExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Touched every time a mirror is opened; its mtime is the "last queried"
/// signal the sweep uses. A dedicated file instead of one of git's own
/// (HEAD, refs, packs) because those only change when the mirror's
/// *content* changes, not on every read-only request that uses it.
#[cfg(any(test, feature = "test-util"))]
pub const LAST_ACCESS_MARKER: &str = ".gitquarry-last-access";
#[cfg(not(any(test, feature = "test-util")))]
const LAST_ACCESS_MARKER: &str = ".gitquarry-last-access";

/// How often the sweep runs. Coarser than `mirror_max_age`'s granularity
/// (days) on purpose: cheap to run, no reason to make this configurable too.
const SWEEP_INTERVAL: Duration = Duration::from_secs(15 * 60);

pub async fn touch_last_access(mirror_path: &Path) -> Result<()> {
    let marker = mirror_path.join(LAST_ACCESS_MARKER);
    tokio::fs::write(&marker, b"")
        .await
        .with_context(|| format!("updating {}", marker.display()))
}

/// Parses `"<number><unit>"` with `unit` in `s`/`m`/`h`/`d` (seconds,
/// minutes, hours, days) — e.g. `"3d"`, `"12h"`.
pub fn parse_age(s: &str) -> Result<Duration> {
    let (num, unit) = split_number_unit(s)?;
    let secs = match unit {
        "s" => num,
        "m" => num.saturating_mul(60),
        "h" => num.saturating_mul(3600),
        "d" => num.saturating_mul(86400),
        other => bail!("invalid time unit '{other}' in '{s}' (use s/m/h/d)"),
    };
    Ok(Duration::from_secs(secs))
}

/// Parses `"<number><unit>"` with `unit` in `B`/`K`/`M`/`G` (bytes, KiB,
/// MiB, GiB; case-insensitive, `B` optional) — e.g. `"5G"`, `"512M"`.
pub fn parse_size(s: &str) -> Result<u64> {
    let (num, unit) = split_number_unit(s)?;
    let bytes = match unit.to_ascii_uppercase().as_str() {
        "" | "B" => num,
        "K" => num.saturating_mul(1024),
        "M" => num.saturating_mul(1024 * 1024),
        "G" => num.saturating_mul(1024 * 1024 * 1024),
        other => bail!("invalid size unit '{other}' in '{s}' (use B/K/M/G)"),
    };
    Ok(bytes)
}

fn split_number_unit(s: &str) -> Result<(u64, &str)> {
    let s = s.trim();
    let split_at = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split_at);
    let num: u64 = num
        .parse()
        .with_context(|| format!("invalid value: '{s}' (expected a number followed by a unit)"))?;
    Ok((num, unit))
}

struct MirrorInfo {
    repo_name: String,
    path: PathBuf,
    last_access: SystemTime,
    size_bytes: u64,
}

/// Runs [`sweep_once`] forever on [`SWEEP_INTERVAL`]. A sweep failure (a
/// transient I/O error walking `mirrors_root`, say) is logged and retried
/// next tick — it must never take down git://, ssh:// or the web UI, so this
/// is meant to run detached (`tokio::spawn`), not inside the same
/// `tokio::select!` as the three servers.
pub async fn run_periodic_sweep(ctx: Arc<AppContext>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(e) = sweep_once(&ctx).await {
            eprintln!("gitquarry: warning: mirror sweep failed: {e:#}");
        }
    }
}

/// How many mirrors [`prewarm_recent_mirrors`] syncs concurrently. Each sync
/// can itself download several packs at once
/// ([`super::DOWNLOAD_CONCURRENCY`]), so this stays modest to keep the
/// total concurrent transfer count reasonable on a busy instance's first
/// boot.
const PREWARM_CONCURRENCY: usize = 4;

/// Re-syncs every mirror still within [`AppContext::mirror_max_age`] of its
/// last access — the ones [`sweep_once`] would keep, not evict — right
/// after startup, so a repo someone was actively using before a restart
/// doesn't make the first request after one pay for a cold sync. Meant to
/// run detached (`tokio::spawn`, same as [`run_periodic_sweep`]): with many
/// mirrors this can mean real pack downloads, and startup accepting
/// connections should never wait on that.
///
/// Best-effort per mirror: one repo's sync failing (a transient backend
/// error, a repo deleted since the marker was last touched) is logged and
/// skipped, never fatal to the rest of the prewarm.
pub async fn prewarm_recent_mirrors(ctx: Arc<AppContext>) {
    let mirrors = match list_mirrors(&ctx.mirrors_root).await {
        Ok(mirrors) => mirrors,
        Err(e) => {
            eprintln!("gitquarry: warning: couldn't list mirrors to prewarm, continuing: {e:#}");
            return;
        }
    };
    let now = SystemTime::now();
    let recent: Vec<MirrorInfo> = mirrors
        .into_iter()
        .filter(|m| now.duration_since(m.last_access).unwrap_or_default() <= ctx.mirror_max_age)
        .collect();
    if recent.is_empty() {
        return;
    }
    eprintln!("gitquarry: prewarming {} recently-used mirror(s)", recent.len());

    stream::iter(recent)
        .for_each_concurrent(Some(PREWARM_CONCURRENCY), |m| {
            let ctx = Arc::clone(&ctx);
            async move {
                // Same per-repo lock `AppContext::open_mirror` takes: a real
                // request for this repo arriving mid-prewarm just waits its
                // turn instead of racing this sync, exactly as it would
                // race a normal open_mirror call from another request.
                let _guard = ctx.locks.lock(&m.repo_name).await;
                let remote = ctx.remote_for_uid(&crate::storage::uid::Uid::from(m.repo_name.clone()));
                if let Err(e) = crate::mirror::sync_bare_repo_from_s3(&remote, &m.path).await {
                    eprintln!("gitquarry: warning: prewarm sync failed for mirror '{}': {e:#}", m.repo_name);
                }
            }
        })
        .await;
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
    let mirrors = list_mirrors(&ctx.mirrors_root).await?;
    let now = SystemTime::now();

    let mut kept = Vec::new();
    for m in mirrors {
        let age = now.duration_since(m.last_access).unwrap_or_default();
        if age > ctx.mirror_max_age {
            evict(ctx, &m).await;
        } else {
            kept.push(m);
        }
    }

    // LRU eviction on top of the survivors, oldest access first, until back
    // under the size cap.
    kept.sort_by_key(|m| m.last_access);
    let mut total: u64 = kept.iter().map(|m| m.size_bytes).sum();
    for m in kept {
        if total <= ctx.mirror_max_size_bytes {
            break;
        }
        total = total.saturating_sub(m.size_bytes);
        evict(ctx, &m).await;
    }
    Ok(())
}

/// Removes a mirror, but only after taking its repo lock and re-checking
/// that it's still stale: a request that arrived while we waited for the
/// lock (held by that same request, doing a fetch/sync) already refreshed
/// the marker, so it isn't actually stale by the time we get in.
async fn evict(ctx: &AppContext, mirror: &MirrorInfo) {
    let _guard = ctx.locks.lock(&mirror.repo_name).await;
    let marker = mirror.path.join(LAST_ACCESS_MARKER);
    let refreshed = tokio::fs::metadata(&marker)
        .await
        .and_then(|m| m.modified())
        .map(|t| t > mirror.last_access)
        .unwrap_or(false);
    if refreshed {
        return;
    }
    match tokio::fs::remove_dir_all(&mirror.path).await {
        Ok(()) => eprintln!(
            "gitquarry: mirror '{}' cleaned from cache ({} bytes freed)",
            mirror.repo_name, mirror.size_bytes
        ),
        Err(e) => eprintln!(
            "gitquarry: warning: couldn't clean up mirror of '{}' ({}): {e:#}",
            mirror.repo_name,
            mirror.path.display()
        ),
    }
}

/// Finds every bare-repo mirror under `mirrors_root` (a directory containing
/// a `HEAD` file — the same check [`crate::mirror::ensure_bare_repo`]
/// uses to decide whether a mirror already exists). Repo names can nest
/// (`team/project`), so this walks the tree iteratively instead of
/// assuming a fixed depth, stopping at the first bare-repo dir found on each
/// branch — its own subdirectories (`objects/`, `refs/`, ...) aren't repos.
async fn list_mirrors(mirrors_root: &Path) -> Result<Vec<MirrorInfo>> {
    let mut out = Vec::new();
    let mut stack = vec![mirrors_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if tokio::fs::try_exists(dir.join("HEAD")).await.unwrap_or(false) {
            let size_bytes = dir_size(&dir).await?;
            let marker = dir.join(LAST_ACCESS_MARKER);
            let last_access = tokio::fs::metadata(&marker)
                .await
                .and_then(|m| m.modified())
                // Never touched (shouldn't happen for anything created via
                // `open_mirror`, but cheap to handle): treat as maximally
                // stale so it's a prime eviction candidate rather than
                // silently exempt from the age check.
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let repo_name = dir
                .strip_prefix(mirrors_root)
                .unwrap_or(&dir)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            out.push(MirrorInfo {
                repo_name,
                path: dir,
                last_access,
                size_bytes,
            });
            continue;
        }
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("reading {}", dir.display()))?
        {
            if entry.file_type().await.is_ok_and(|ft| ft.is_dir()) {
                stack.push(entry.path());
            }
        }
    }
    Ok(out)
}

async fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&d)
            .await
            .with_context(|| format!("reading {}", d.display()))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("reading {}", d.display()))?
        {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if let Ok(metadata) = entry.metadata().await {
                total += metadata.len();
            }
        }
    }
    Ok(total)
}
