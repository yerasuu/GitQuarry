//! What one client may cost `/_api/v1` (decision #144): a request rate per
//! token, a rate of failed authentications per client address, and the
//! `RateLimit-*` fields that tell a client where it stands, as
//! [draft-ietf-httpapi-ratelimit-headers-06](https://datatracker.ietf.org/doc/html/draft-ietf-httpapi-ratelimit-headers-06)
//! names them.
//!
//! The values come from `[api]` in `gitquarry.toml`
//! ([`crate::config::ApiSection`]); each has a ceiling here that no
//! configuration raises, so a typo'd extra zero can't open the server up.
//!
//! Counters live in memory, in fixed one-minute windows, and a restart
//! clears them, as with the federation limiter (decision #69) and
//! [`crate::app::GitRateLimiter`]. A fixed window lets a client spend up to
//! twice its rate across a window boundary; in exchange each key costs one
//! counter instead of a timestamp per request, and the `RateLimit-Reset` a
//! client reads is the moment its whole quota comes back.
//!
//! Not enforced yet: decision #144 also asks for a mandatory path on log,
//! blame and search over repos above a size threshold. No endpoint reads
//! those yet. The first one to do it calls
//! [`super::auth::ApiPrincipal::require_path`] with the path it was given,
//! refuses an empty one with `invalid_parameter` once the repo is over the
//! threshold, and the threshold joins `[api]` next to these, with its own
//! ceiling.

use super::error::{ApiError, ErrorCode};
use crate::app::AppContext;
use crate::config::ApiSection;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Extensions, HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

pub const REQUESTS_PER_MINUTE_CEILING: u32 = 1200;
pub const FAILED_AUTH_PER_MINUTE_CEILING: u32 = 300;

const WINDOW: Duration = Duration::from_secs(60);

/// Keys one table holds at most. Token keys are bounded by the tokens that
/// exist, but failure keys are client addresses, which a client with a
/// large address block mints at will. Past this a new key is refused as if
/// it were over its limit: failing closed only costs fresh addresses their
/// failed attempts, while failing open would leave such a client unlimited.
const MAX_KEYS: usize = 100_000;

/// Where one key stands in its current window, as the `RateLimit-*` fields
/// report it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Quota {
    limit: u32,
    remaining: u32,
    reset: u64,
    exceeded: bool,
}

impl Quota {
    /// Whether the request this quota was read for was refused.
    pub(crate) fn exceeded(&self) -> bool {
        self.exceeded
    }

    /// Writes `RateLimit-Limit`, `RateLimit-Remaining`, `RateLimit-Reset`
    /// and `RateLimit-Policy`, and on a `429` a `Retry-After` for the same
    /// moment as `RateLimit-Reset`, which the draft asks for when both are
    /// sent.
    fn write_headers(&self, headers: &mut HeaderMap, refused: bool) {
        headers.insert("ratelimit-limit", HeaderValue::from(self.limit));
        headers.insert("ratelimit-remaining", HeaderValue::from(self.remaining));
        headers.insert("ratelimit-reset", HeaderValue::from(self.reset));
        if let Ok(policy) = HeaderValue::from_str(&format!("{};w={}", self.limit, WINDOW.as_secs())) {
            headers.insert("ratelimit-policy", policy);
        }
        if refused {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(self.reset));
        }
    }
}

#[derive(Debug)]
struct Window {
    started: Instant,
    count: u32,
}

#[derive(Debug, Default)]
struct Table {
    by_key: HashMap<String, Window>,
    last_sweep: Option<Instant>,
}

#[derive(Debug, Default)]
struct Windows {
    inner: Mutex<Table>,
}

/// Whole seconds until `started`'s window ends, rounded up so a client that
/// waits that long is never early.
fn seconds_left(started: Instant, now: Instant) -> u64 {
    let left = WINDOW.saturating_sub(now.duration_since(started));
    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

impl Windows {
    /// A poisoned lock only means another request panicked mid-update; the
    /// counters are still a valid, if slightly off, limit.
    fn lock(&self) -> MutexGuard<'_, Table> {
        let mut table = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        // At most once per window, so the map holds only keys seen in about
        // the last two windows.
        if table.last_sweep.is_none_or(|last| now.duration_since(last) >= WINDOW) {
            table
                .by_key
                .retain(|_, window| now.duration_since(window.started) < WINDOW);
            table.last_sweep = Some(now);
        }
        table
    }

    /// Counts one request against `key` if it is under `max`; a refused
    /// request is not counted, so a client at its limit gets its full
    /// quota back when the window ends.
    fn hit(&self, key: &str, max: u32) -> Quota {
        let mut table = self.lock();
        let now = Instant::now();
        if !table.by_key.contains_key(key) && table.by_key.len() >= MAX_KEYS {
            return full_table(max);
        }
        let window = table
            .by_key
            .entry(key.to_string())
            .or_insert(Window { started: now, count: 0 });
        if now.duration_since(window.started) >= WINDOW {
            *window = Window { started: now, count: 0 };
        }
        let exceeded = window.count >= max;
        if !exceeded {
            window.count += 1;
        }
        Quota {
            limit: max,
            remaining: max.saturating_sub(window.count),
            reset: seconds_left(window.started, now),
            exceeded,
        }
    }

    /// Where `key` stands, counting nothing. `exceeded` means a [`Self::hit`]
    /// now would be refused.
    fn peek(&self, key: &str, max: u32) -> Quota {
        let table = self.lock();
        let now = Instant::now();
        match table.by_key.get(key) {
            Some(window) if now.duration_since(window.started) < WINDOW => Quota {
                limit: max,
                remaining: max.saturating_sub(window.count),
                reset: seconds_left(window.started, now),
                exceeded: window.count >= max,
            },
            Some(_) => fresh(max),
            None if table.by_key.len() >= MAX_KEYS => full_table(max),
            None => fresh(max),
        }
    }
}

fn fresh(max: u32) -> Quota {
    Quota {
        limit: max,
        remaining: max,
        reset: WINDOW.as_secs(),
        exceeded: false,
    }
}

fn full_table(max: u32) -> Quota {
    Quota {
        limit: max,
        remaining: 0,
        reset: WINDOW.as_secs(),
        exceeded: true,
    }
}

/// The API's limits and their counters, one per process, held by
/// [`crate::app::AppContext::api_limits`].
#[derive(Debug)]
pub struct ApiLimits {
    requests_per_minute: u32,
    failed_auth_per_minute: u32,
    by_token: Windows,
    failures_by_client: Windows,
}

impl ApiLimits {
    /// Limits from `section`. A value outside its range is brought inside
    /// it here as well as refused by [`crate::config::Config::load_from`],
    /// so a section built in code can't exceed a ceiling either.
    pub fn new(section: &ApiSection) -> Self {
        ApiLimits {
            requests_per_minute: section.requests_per_minute.clamp(1, REQUESTS_PER_MINUTE_CEILING),
            failed_auth_per_minute: section.failed_auth_per_minute.clamp(1, FAILED_AUTH_PER_MINUTE_CEILING),
            by_token: Windows::default(),
            failures_by_client: Windows::default(),
        }
    }

    /// Counts one request by the token with this uid. Keyed by uid, never
    /// by secret, so nothing secret sits in memory and every spelling of
    /// the `Authorization` header that opens the token shares its quota.
    pub(crate) fn hit_token(&self, token_uid: &str) -> Quota {
        self.by_token.hit(token_uid, self.requests_per_minute)
    }

    /// Counts one missing or refused credential from `client`.
    pub(crate) fn record_failure(&self, client: &str) -> Quota {
        self.failures_by_client.hit(client, self.failed_auth_per_minute)
    }

    /// Where `client` stands on failed credentials; checked before a token
    /// is looked up, so a client past it costs no storage read.
    pub(crate) fn failures(&self, client: &str) -> Quota {
        self.failures_by_client.peek(client, self.failed_auth_per_minute)
    }
}

impl Default for ApiLimits {
    fn default() -> Self {
        ApiLimits::new(&ApiSection::default())
    }
}

/// The address failed credentials are counted against, resolved as
/// git-smart-HTTP resolves it for its anonymous limit (`trust_forwarded_for`,
/// see #20). Served without `ConnectInfo`, every client falls into one
/// shared bucket: the limit then gets stricter, never absent.
pub(crate) fn client(ctx: &AppContext, headers: &HeaderMap, extensions: &Extensions) -> String {
    let socket = extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |info| info.0.ip());
    crate::git::http::client_key(ctx, headers, socket)
}

pub(crate) fn rate_limited(message: &'static str) -> ApiError {
    ApiError::new(ErrorCode::RateLimited, message)
        .with_hint("wait the seconds `Retry-After` gives, then retry; `RateLimit-Remaining` counts what is left")
}

/// Where [`super::auth::ApiPrincipal`] leaves the token's quota for
/// [`middleware`] to report. Set at most once per request.
#[derive(Clone, Default)]
pub(crate) struct TokenQuota(Arc<OnceLock<Quota>>);

impl TokenQuota {
    pub(crate) fn set(&self, quota: Quota) {
        // A second principal in one request would be counted twice; the
        // first quota is the one its headers already describe.
        let _ = self.0.set(quota);
    }
}

/// Puts the `RateLimit-*` fields on every `/_api/v1` response: the token's
/// quota once a token was accepted, else the client address's quota of
/// failed credentials, which is the one a request without a usable token
/// spends.
pub(crate) async fn middleware(State(ctx): State<Arc<AppContext>>, mut request: Request, next: Next) -> Response {
    let client = client(&ctx, request.headers(), request.extensions());
    let slot = TokenQuota::default();
    request.extensions_mut().insert(slot.clone());
    let mut response = next.run(request).await;
    let quota = slot
        .0
        .get()
        .copied()
        .unwrap_or_else(|| ctx.api_limits.failures(&client));
    let refused = response.status() == StatusCode::TOO_MANY_REQUESTS;
    quota.write_headers(response.headers_mut(), refused);
    response
}
