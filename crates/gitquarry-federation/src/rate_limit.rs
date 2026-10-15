//! Per-peer inbound rate limiting (section 6) — two independent buckets,
//! same in-memory sliding-window shape the single-binary predecessor's
//! `LoginAttempts` already used for failed-login tracking: simple, resets
//! on restart, no new dependency (this workspace has no
//! `governor`/`tower::limit` crate today).
//!
//! **The caller IP this keys "public" requests by is never a
//! cryptographic guarantee.** This binary's own loopback bind
//! (`gitquarry-federation.toml`'s `listen`) is never reachable except
//! through `gitquarry-core`'s proxy (`webui::federation_proxy`'s own doc
//! comment) — core forwards the real client address as
//! `X-Forwarded-For`, and every rate-limit check here trusts that header
//! over its own `ConnectInfo` (which would otherwise just see core's own
//! loopback address on every request, useless for this purpose). Same
//! trust boundary as `X-Internal-Token` itself: this only works because
//! core is the sole caller of this bind address at all.
//!
//! `Peer::rate_limit_override` attribution (matching a caller's IP against
//! a *linked* peer's own `web_base_url`/`git_base_url` host) is a
//! heuristic on top of that already-approximate IP, not a second
//! cryptographic check — see the open question in
//! `docs/federation-binary-split-concept.md`'s own closing section.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Attempts {
    by_key: HashMap<String, Vec<Instant>>,
    last_sweep: Option<Instant>,
}

#[derive(Default)]
struct Bucket {
    inner: Mutex<Attempts>,
}

impl Bucket {
    /// `true` if `key` is still under `max` requests within `window` —
    /// records this attempt either way isn't right: only a request that's
    /// actually let through counts against the window, otherwise a caller
    /// stuck at the ceiling would never recover once new attempts keep
    /// evicting old ones from the retained set. Records only on success.
    fn check(&self, key: &str, max: u32, window: Duration) -> bool {
        let now = Instant::now();
        let mut inner = self.inner.lock().expect("rate limit bucket lock poisoned");
        // Swept at most once per window so the map only ever holds keys seen
        // within roughly the last two windows — otherwise every distinct
        // caller IP ever seen stays resident until restart.
        if inner.last_sweep.is_none_or(|last| now.duration_since(last) >= window) {
            inner.by_key.retain(|_, attempts| {
                attempts.retain(|t| now.duration_since(*t) < window);
                !attempts.is_empty()
            });
            inner.last_sweep = Some(now);
        }
        let attempts = inner.by_key.entry(key.to_string()).or_default();
        attempts.retain(|t| now.duration_since(*t) < window);
        if attempts.len() as u32 >= max {
            return false;
        }
        attempts.push(now);
        true
    }
}

/// The two independent buckets section 6 calls for — public/unauthenticated
/// (keyed by caller IP) and authenticated (keyed by resolved token owner).
/// A peer's own `rate_limit_override` (looked up by the caller before
/// calling [`Self::check_public`]) replaces `max` for that one check, never
/// `window` — only the ceiling is peer-specific.
#[derive(Default)]
pub struct PeerRateLimiter {
    public: Bucket,
    token: Bucket,
}

impl PeerRateLimiter {
    pub fn check_public(&self, key: &str, max: u32, window: Duration) -> bool {
        self.public.check(key, max, window)
    }

    pub fn check_token(&self, key: &str, max: u32, window: Duration) -> bool {
        self.token.check(key, max, window)
    }
}
