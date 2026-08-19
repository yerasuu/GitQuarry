//! Wires `gitquarry-mirror-external`'s sync engine into
//! `gitquarry_core::mirror::MirrorSyncHook` — core defines the hook point,
//! this crate (the one place that depends on both core and
//! gitquarry-mirror-external) supplies the real implementation at startup.
//! See `docs/mirror-external-extraction-concept.md`.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::mirror::MirrorSyncHook;
use std::time::Duration;

/// Bound on [`MirrorExternalHook::sync_before_serving`]'s wait — an
/// external forge being slow or unreachable must never turn into an
/// unbounded hang on a clone/fetch/push, the single most latency-sensitive
/// path in core. Any timeout or error just logs and lets the caller serve
/// with whatever the local mirror already has.
const SYNC_BEFORE_SERVE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct MirrorExternalHook;

#[async_trait::async_trait]
impl MirrorSyncHook for MirrorExternalHook {
    async fn sync_before_serving(&self, ctx: &AppContext, resolved: &ResolvedRepo) {
        match tokio::time::timeout(
            SYNC_BEFORE_SERVE_TIMEOUT,
            gitquarry_mirror_external::engine::sync_repo_mirrors(ctx, resolved),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                eprintln!(
                    "gitquarry: warning: pre-serve mirror sync for {} failed, serving with what's local: {e:#}",
                    resolved.uid
                );
            }
            Err(_elapsed) => {
                eprintln!(
                    "gitquarry: warning: pre-serve mirror sync for {} timed out after {SYNC_BEFORE_SERVE_TIMEOUT:?}, \
                     serving with what's local",
                    resolved.uid
                );
            }
        }
    }
}
