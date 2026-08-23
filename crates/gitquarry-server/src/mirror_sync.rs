//! Wires `gitquarry-mirror-external`'s sync engine into
//! `gitquarry_core::mirror::MirrorSyncHook` — core defines the hook point,
//! this crate (the one place that depends on both core and
//! gitquarry-mirror-external) supplies the real implementation at startup.
//! See `docs/mirror-external-extraction-concept.md`.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::mirror::MirrorSyncHook;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::warn;

/// Bound on [`MirrorExternalHook::sync_before_serving`]'s wait — an
/// external forge being slow or unreachable must never turn into an
/// unbounded hang on a clone/fetch/push, the single most latency-sensitive
/// path in core. Any timeout or error just logs and lets the caller serve
/// with whatever the local mirror already has.
const SYNC_BEFORE_SERVE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a pre-serve sync counts as "recent enough" for one repo.
///
/// Core calls this hook on every `info/refs`, `upload-pack` and
/// `receive-pack` — so a single `git clone` over HTTP is *two* requests and
/// used to mean two full sweeps back to back, and a repo under CI traffic
/// hammered its external forge in proportion to how often it was cloned.
/// The sweep exists to keep a served ref reasonably fresh, and a minute of
/// staleness is well inside what the periodic tick's own 15-minute cadence
/// already accepts.
const MIN_INTERVAL_BETWEEN_SERVES: Duration = Duration::from_secs(60);

/// Last pre-serve sweep per repo uid. In-process and deliberately not
/// persisted: this exists to protect *this* process's git latency and the
/// request burst one client produces, and a second instance throttling on
/// its own clock is the right behaviour, not a coordination problem.
/// Bounded by the number of repos with an enabled mirror, which is
/// operator-curated — no eviction needed.
#[derive(Default)]
pub struct MirrorExternalHook {
    last_serve: Mutex<HashMap<String, Instant>>,
}

impl MirrorExternalHook {
    /// `true` if enough time has passed to sweep again — and, if so,
    /// records this attempt. Stamped before the sweep rather than after, so
    /// a slow forge can't let a burst of requests queue up behind it and
    /// then each start their own.
    fn claim_slot(&self, uid: &str) -> bool {
        let now = Instant::now();
        let mut last = self.last_serve.lock().expect("mirror pre-serve clock poisoned");
        match last.get(uid) {
            Some(at) if now.duration_since(*at) < MIN_INTERVAL_BETWEEN_SERVES => false,
            _ => {
                last.insert(uid.to_string(), now);
                true
            }
        }
    }
}

#[async_trait::async_trait]
impl MirrorSyncHook for MirrorExternalHook {
    async fn sync_before_serving(&self, ctx: &AppContext, resolved: &ResolvedRepo) {
        if !self.claim_slot(resolved.uid.as_str()) {
            return;
        }
        match tokio::time::timeout(
            SYNC_BEFORE_SERVE_TIMEOUT,
            gitquarry_mirror_external::engine::sync_repo_mirrors(ctx, resolved),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(
                    "pre-serve mirror sync for {} failed, serving with what's local: {e:#}",
                    resolved.uid
                );
            }
            Err(_elapsed) => {
                warn!(
                    "pre-serve mirror sync for {} timed out after {SYNC_BEFORE_SERVE_TIMEOUT:?}, \
                     serving with what's local",
                    resolved.uid
                );
            }
        }
    }
}
