//! Shared state for the ssh:// server and the web UI (browsing + git
//! smart-HTTP).
//!
//! Both run in the same process and work over the same bucket and the same
//! local mirrors, so they share a single `AppContext`: one storage backend
//! (cheaply `Clone`, brings its own connection pool) and one lock registry,
//! without which a clone and a UI render could touch the same mirror at the
//! same time.

use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::remote::Remote;
use crate::storage::uid::Uid;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock as AsyncRwLock};
use tracing::warn;

tokio::task_local! {
    /// Per-request `s3db::RequestCache`-wrapped backend, scoped by
    /// `webui::request_cache_middleware` for the lifetime of one HTTP
    /// request — so a repeat read of the same key within one page render
    /// (the session's own `User`, `ServerSettings`, a repo's `RepoConfig`,
    /// ...) is served from memory instead of a second real S3 round trip.
    /// [`AppContext::backend`] reads this first, falling back to the
    /// shared long-lived backend outside any request (background sweeps,
    /// ssh:// -- which already has its own separate per-connection cache,
    /// see `git::ssh::ConnHandler::cache`). Deliberately task-local, not
    /// process-wide, for the same reason `RequestCache`'s own doc comment
    /// gives: gitquarry runs multiple instances against the same bucket, a
    /// cache that outlived one request could let a revoked permission
    /// stay "live" longer than it should.
    pub static REQUEST_BACKEND: Arc<dyn Backend>;
}

#[derive(Debug)]
pub struct AppContext {
    pub bucket: String,
    /// The address the ssh:// server actually bound at startup (CLI flag >
    /// remote settings > default — see `crate::serve::run`), regardless of
    /// whether that value ever got persisted to `.conf/global.json`. The
    /// web UI's clone-URL widget reads this, not the remote settings
    /// directly — most deployments never save `ssh_bind` there at all
    /// (running on the CLI default is the common case), and deriving the
    /// clone instructions from a field that's usually unset would silently
    /// drop the ssh:// row from a repo whose ssh:// access works fine.
    pub ssh_bind: String,
    pub mirrors_root: PathBuf,
    /// Absolute path to the config: propagated to git hooks, which run with
    /// a different cwd and wouldn't resolve it the same way.
    pub config_path: PathBuf,
    /// Absolute path to this binary, so the `pre-receive` hook doesn't
    /// depend on `gitquarry` being in PATH.
    pub exe_path: PathBuf,
    pub backend: Arc<dyn Backend>,
    pub locks: Locks,
    /// Encrypts everything this process uploads to the bucket. Derived once
    /// at startup (see [`crate::crypto::bootstrap::get_or_create_cipher`])
    /// and shared between the ssh:// server and the web UI (browsing + git
    /// smart-HTTP).
    pub cipher: Arc<Cipher>,
    /// A mirror not queried in longer than this gets evicted by
    /// [`crate::mirror::gc::run_periodic_sweep`].
    pub mirror_max_age: Duration,
    /// Soft cap on `mirrors_root`'s total size: once age-based eviction
    /// runs, least-recently-queried survivors get evicted until back under
    /// this, if still over it.
    pub mirror_max_size_bytes: u64,
    /// A mirror at or above this size is exempt from age-based eviction —
    /// it still fully participates in the size-based pass, so
    /// `mirror_max_size_bytes` is never bypassed. Always has a value (no
    /// external wiring needed to be safe, unlike `oidc`/`mirror_sync`): it
    /// only ever makes eviction more conservative.
    pub mirror_large_repo_threshold_bytes: u64,
    /// `None` unless all four `oidc_*` settings are configured — see
    /// [`gitquarry_auth_crypto::oidc::discover`]. Web UI admin pages and
    /// reading `private` repos over the web both stay exactly as open as
    /// they've always been when this is `None`.
    pub oidc: Option<gitquarry_auth_crypto::oidc::OidcClient>,
    /// Failed-password-login tracker for `auth::web::login`'s local password
    /// form — see [`LoginAttempts`].
    pub login_attempts: LoginAttempts,
    /// `None` unless `[federation]` is configured — see
    /// [`FederationLink`]. Every outbound proxy route in `webui` checks
    /// this before forwarding anything; absent, those routes simply don't
    /// exist (see `config::FederationSection`'s own doc comment on the
    /// "`None` = feature fully off" pattern).
    pub federation: Option<FederationLink>,
    /// anything, same "`None` = feature fully off" pattern
    /// `federation` already establishes:
    /// `config::Config::allow_push_create` — gates the ssh:// receive-pack
    /// handler's implicit "first push creates the repo" branch. `false` by
    /// default (see that field's doc comment for why).
    pub allow_push_create: bool,
    /// `config::Config::trust_forwarded_for` — whether `git::http`'s
    /// anonymous rate limiter may key on `X-Forwarded-For` instead of the
    /// socket address. `false` by default; see that field for why it can't
    /// be on unless a proxy owns the header.
    pub trust_forwarded_for: bool,
    /// Clone/fetch flood tracker for `git::http`'s `info/refs`/`upload-pack`
    /// handlers — see [`GitRateLimiter`].
    pub git_rate_limiter: GitRateLimiter,
    /// Fan-out point for domain state changes (push, MR, issue, release) —
    /// see [`crate::events`]. No consumers registered yet.
    pub events: crate::events::EventBus,
    /// `None` unless `gitquarry-server` wired one in at startup — see
    /// [`crate::mirror::MirrorSyncHook`]'s own doc comment for why this is
    /// a trait object rather than a direct dependency on
    /// `gitquarry-mirror-external`.
    pub mirror_sync: Option<Arc<dyn crate::mirror::MirrorSyncHook>>,
    /// Short-TTL cache for [`Self::visible_repo_uids`] — see that method.
    pub visible_repo_uids_cache: VisibleRepoUidsCache,
}

/// What core needs to reach `gitquarry-federation`'s own loopback API —
/// the *outbound* half of the IPC boundary (the *inbound* half,
/// `gitquarry-core`'s own `/_internal/*` server, is [`crate::internal_api::run_server`]).
#[derive(Debug, Clone)]
pub struct FederationLink {
    /// `gitquarry-federation.toml`'s `listen` address, as a full
    /// `http://host:port` base URL — `config::FederationSection::federation_url`.
    pub base_url: String,
    /// Shared secret sent as `X-Internal-Token` on every proxied request —
    /// must equal federation's own `internal_token`.
    pub internal_token: String,
    pub http: reqwest::Client,
}

/// Backs [`AppContext::visible_repo_uids`]: the anonymous repo index (the
/// site's highest-traffic page) lists every visible repo uid on every
/// render, which is a real `list_prefix` call against the backend —
/// `s3db::CachedBackend` (wired in by [`crate::storage::build_backend`])
/// forwards list operations to the driver uncached, it only caches point
/// `get`s. A few seconds of staleness here is the same "hint, not a fact"
/// tolerance [`crate::storage::settings::list_visible_repo_uids`]'s own doc
/// comment already describes — a repo that just turned visible (or private)
/// shows up a few seconds late either way, self-heals on the next read past
/// the TTL, and `webui::repo::list_visible_repos` re-checks each uid's real
/// `RepoConfig::visibility` regardless, so this can never do worse than that
/// existing lag.
#[derive(Debug, Default)]
pub struct VisibleRepoUidsCache {
    inner: Mutex<Option<(Instant, Vec<Uid>)>>,
}

const VISIBLE_REPO_UIDS_TTL: Duration = Duration::from_secs(10);

/// Local mirror ready to use, with the repo's lock held — exclusively when
/// the mirror had to be created or synced, shared when it was already at
/// the manifest the bucket holds (see [`AppContext::open_mirror_read`]).
#[derive(Debug)]
pub struct Mirror {
    pub remote: Remote,
    pub path: PathBuf,
    _guard: MirrorGuard,
}

/// A linked peer's specific repo, synced via a real `git fetch` against the
/// peer's own URL instead of S3 — federation layer 2
/// (`docs/federation-concept.md`), [`AppContext::open_remote_mirror`]'s
/// return type. No `remote: Remote` field like [`Mirror`] has: there's no
/// local S3 storage for this content until someone forks it (not built
/// yet) — only ever a disposable local git mirror, same as [`Mirror`]'s
/// `path` but with nothing backing it in this bucket.
#[derive(Debug)]
pub struct PeerMirror {
    pub path: PathBuf,
    _guard: OwnedRwLockWriteGuard<()>,
}

/// Which half of [`Locks`] a live [`Mirror`] is holding. An enum rather than
/// two `Mirror` types because every caller uses the mirror identically; all
/// that differs is whether anyone else may be reading it at the same time.
/// Whether an `open_mirror_common` call is somebody actually querying the
/// repo or the startup prewarm refilling a cold cache. Only a query stamps
/// the last-access marker and the bucket-side recent index: a prewarm that
/// stamped them would keep an idle repo warm forever, one restart at a time.
#[derive(Debug, Clone, Copy)]
enum Access {
    Query,
    Warm,
}

#[derive(Debug)]
enum MirrorGuard {
    Shared(#[allow(dead_code)] OwnedRwLockReadGuard<()>),
    Exclusive(#[allow(dead_code)] OwnedRwLockWriteGuard<()>),
}

/// A lock per repo. The local mirror (`mirrors_root/<repo>`) is shared
/// state, but only *mutating* it conflicts: `git init`, a pack download and
/// `update-ref` must not run twice over the same directory at once, while
/// any number of `git log`/`ls-tree`/`upload-pack` readers over an
/// unchanging directory are fine together.
///
/// Hence a read/write lock rather than the plain mutex this used to be:
/// under that mutex a page holding the lock for its whole render blocked
/// every other view of the same repo, measured at 11ms → 920ms for a fast
/// page opened while a slow one was in flight.
#[derive(Debug, Default)]
pub struct Locks {
    inner: Mutex<HashMap<String, Arc<AsyncRwLock<()>>>>,
}

impl Locks {
    /// Exclusive: nothing else touches this repo's mirror while the guard
    /// lives. What every mutation (push, sync, eviction, MR merge) takes.
    pub async fn lock(&self, repo: &str) -> OwnedRwLockWriteGuard<()> {
        self.entry(repo).write_owned().await
    }

    /// Shared: other readers of the same mirror proceed concurrently, no
    /// writer can start. Only safe once the mirror is known to already
    /// match the bucket — see [`AppContext::open_mirror_read`].
    pub async fn lock_shared(&self, repo: &str) -> OwnedRwLockReadGuard<()> {
        self.entry(repo).read_owned().await
    }

    fn entry(&self, repo: &str) -> Arc<AsyncRwLock<()>> {
        // The sync Mutex only protects the HashMap; it's never held across
        // an await, so it can't deadlock with the async one.
        let mut map = self.inner.lock().expect("locks mutex poisoned");
        // Repos nobody is using (only the map's own ref left) get dropped:
        // without this the map grows with every distinct path requested.
        map.retain(|_, m| Arc::strong_count(m) > 1);
        Arc::clone(map.entry(repo.to_string()).or_default())
    }
}

/// Per-username failed-password-login tracker for `auth::web::login`'s local
/// password form — in-memory only, a restart clears it, same as every other
/// session-related thing in this process. Defense-in-depth against
/// brute-forcing one specific account's password: Argon2id already makes
/// each guess expensive, this caps how many guesses a window allows
/// regardless. Keyed by the submitted username, not by client IP — this
/// app's own threat model already treats its ports as behind a
/// proxy/trusted network (see `webui::set_cookie`'s doc comment), where the
/// TCP peer address usually isn't the real client's anyway.
#[derive(Debug, Default)]
pub struct LoginAttempts {
    inner: Mutex<HashMap<String, Vec<Instant>>>,
}

const LOGIN_MAX_ATTEMPTS: usize = 5;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Ceiling on [`LoginAttempts::penalty`]. Long enough that guessing is
/// hopeless, short enough that a pile of stalled attempts can't be turned
/// into a connection-exhaustion attack of its own.
const LOGIN_MAX_PENALTY: Duration = Duration::from_secs(10);

impl LoginAttempts {
    /// How long a caller must wait before this attempt is even looked at.
    /// Zero until [`LOGIN_MAX_ATTEMPTS`] failures inside the window, then
    /// doubling per further failure up to [`LOGIN_MAX_PENALTY`].
    ///
    /// Deliberately a delay rather than the flat rejection this used to be.
    /// A hard lockout keyed on the username alone is a denial of service
    /// anyone can aim at anyone: five wrong guesses against a name you know
    /// locked its real owner out for the rest of the window. A delay costs
    /// a guesser exactly as much (a few attempts per minute is not a brute
    /// force) while still letting the account's actual owner in.
    pub fn penalty(&self, username: &str) -> Duration {
        let map = self.inner.lock().expect("login-attempts mutex poisoned");
        let now = Instant::now();
        let recent = map.get(username).map_or(0, |attempts| {
            attempts
                .iter()
                .filter(|t| now.duration_since(**t) < LOGIN_WINDOW)
                .count()
        });
        Self::penalty_for(recent)
    }

    fn penalty_for(recent_failures: usize) -> Duration {
        let Some(over) = recent_failures.checked_sub(LOGIN_MAX_ATTEMPTS) else {
            return Duration::ZERO;
        };
        let secs = 1u64.checked_shl(over.min(16) as u32).unwrap_or(u64::MAX);
        Duration::from_secs(secs).min(LOGIN_MAX_PENALTY)
    }

    /// Records one failed attempt, and opportunistically prunes every
    /// username's expired timestamps while the lock is held anyway — same
    /// amortized cleanup [`Locks::lock`] does, so this map never grows
    /// unbounded from distinct usernames tried once and never again.
    pub fn record_failure(&self, username: &str) {
        let mut map = self.inner.lock().expect("login-attempts mutex poisoned");
        let now = Instant::now();
        map.retain(|_, attempts| {
            attempts.retain(|t| now.duration_since(*t) < LOGIN_WINDOW);
            !attempts.is_empty()
        });
        map.entry(username.to_string()).or_default().push(now);
    }

    pub fn clear(&self, username: &str) {
        self.inner
            .lock()
            .expect("login-attempts mutex poisoned")
            .remove(username);
    }

    /// How many distinct usernames are currently paying a penalty — for
    /// `webui::health`'s `/_metrics` gauge, a cheap operational signal that
    /// something (a brute-force attempt, or just someone repeatedly
    /// mistyping their own password) is actively tripping the limiter.
    pub fn locked_count(&self) -> usize {
        let map = self.inner.lock().expect("login-attempts mutex poisoned");
        let now = Instant::now();
        map.values()
            .filter(|attempts| {
                attempts
                    .iter()
                    .filter(|t| now.duration_since(**t) < LOGIN_WINDOW)
                    .count()
                    >= LOGIN_MAX_ATTEMPTS
            })
            .count()
    }
}

/// Clone/fetch flood + scraping tracker for `git::http`'s `info/refs`/
/// `upload-pack` handlers (a plain `git clone`/`fetch` over the smart-HTTP
/// transport) — same in-memory sliding-window shape [`LoginAttempts`]
/// already uses, just two independent buckets instead of one: an
/// authenticated caller (resolved username, from a pubkey or HTTP basic
/// auth) gets a higher ceiling than an anonymous one (keyed by client IP,
/// same "not a cryptographic guarantee, good enough as a heuristic behind
/// a trusted proxy" caveat `webui::set_cookie`'s own doc comment already
/// makes for IP-based checks in this codebase). `ssh://` isn't covered
/// here: every `ssh://` connection is already authenticated by construction
/// (no anonymous git+ssh in this codebase), so its own per-account abuse
/// case is thinner, and — unlike smart-HTTP, which shares this app's one
/// HTTP port and middleware stack — it would need its own separate
/// plumbing to reach the same check.
#[derive(Debug, Default)]
pub struct GitRateLimiter {
    by_ip: Mutex<HashMap<String, Vec<Instant>>>,
    by_user: Mutex<HashMap<String, Vec<Instant>>>,
}

const GIT_RATE_LIMIT_ANONYMOUS_MAX: usize = 30;
const GIT_RATE_LIMIT_AUTHENTICATED_MAX: usize = 120;
const GIT_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

fn rate_limit_check(map: &Mutex<HashMap<String, Vec<Instant>>>, key: &str, max: usize) -> bool {
    let mut map = map.lock().expect("git rate limiter mutex poisoned");
    let now = Instant::now();
    // Same amortized "prune every key's expired timestamps while the lock
    // is held anyway" cleanup `LoginAttempts::record_failure` already
    // does, so this never grows unbounded from one-off IPs/usernames.
    map.retain(|_, attempts| {
        attempts.retain(|t| now.duration_since(*t) < GIT_RATE_LIMIT_WINDOW);
        !attempts.is_empty()
    });
    let attempts = map.entry(key.to_string()).or_default();
    if attempts.len() >= max {
        return false;
    }
    attempts.push(now);
    true
}

impl GitRateLimiter {
    /// `true` if `ip` is still under [`GIT_RATE_LIMIT_ANONYMOUS_MAX`] clone/
    /// fetch requests within the current window — checked before opening
    /// the local mirror, so a caller already at the ceiling doesn't cost
    /// anything extra.
    pub fn check_anonymous(&self, ip: &str) -> bool {
        rate_limit_check(&self.by_ip, ip, GIT_RATE_LIMIT_ANONYMOUS_MAX)
    }

    /// Same as [`Self::check_anonymous`], keyed by the already-resolved
    /// username instead, at [`GIT_RATE_LIMIT_AUTHENTICATED_MAX`].
    pub fn check_authenticated(&self, username: &str) -> bool {
        rate_limit_check(&self.by_user, username, GIT_RATE_LIMIT_AUTHENTICATED_MAX)
    }

    /// How many distinct IPs/usernames are currently at the ceiling — for
    /// `webui::health`'s `/_metrics` gauge, same operational-signal
    /// reasoning as [`LoginAttempts::locked_count`].
    pub fn limited_count(&self) -> usize {
        let now = Instant::now();
        let count_limited = |map: &Mutex<HashMap<String, Vec<Instant>>>, max: usize| {
            map.lock()
                .expect("git rate limiter mutex poisoned")
                .values()
                .filter(|attempts| {
                    attempts
                        .iter()
                        .filter(|t| now.duration_since(**t) < GIT_RATE_LIMIT_WINDOW)
                        .count()
                        >= max
                })
                .count()
        };
        count_limited(&self.by_ip, GIT_RATE_LIMIT_ANONYMOUS_MAX)
            + count_limited(&self.by_user, GIT_RATE_LIMIT_AUTHENTICATED_MAX)
    }
}

/// A repo resolved to its stable identity — the uid that's actually its S3
/// storage prefix (`repos/<uid>/...`), plus a [`Remote`] already built
/// against that prefix. Everything past [`AppContext::resolve_repo`]/
/// [`AppContext::create_repo`] operates on this, never on the display name
/// again: the name was only ever needed to find this in the first place
/// (via the `.conf/index/repo-name/...` index), and nothing downstream —
/// git data, the local mirror, the lock — is keyed by it, which is what
/// makes renaming a repo a pure metadata update (see
/// `webui::repo_settings::move_repo`) instead of relocating any data.
pub struct ResolvedRepo {
    pub uid: Uid,
    pub remote: Remote,
}

impl AppContext {
    /// The backend to actually read/write through: inside a web request,
    /// the per-request `RequestCache`-wrapped one scoped by
    /// `webui::request_cache_middleware` (see [`REQUEST_BACKEND`]); outside
    /// one, the shared long-lived backend below. Everywhere in this codebase
    /// that used to read the `backend` field directly now calls this
    /// instead, so a `Remote` built from `resolve_repo`/`create_repo`/
    /// `remote_for_uid` — and every direct `ctx.backend()` call in
    /// `web`/`domain` — automatically benefits within one request without
    /// needing to know whether it's in one.
    pub fn backend(&self) -> Arc<dyn Backend> {
        REQUEST_BACKEND
            .try_with(Arc::clone)
            .unwrap_or_else(|_| Arc::clone(&self.backend))
    }

    /// Every repo uid currently indexed as visible
    /// ([`crate::storage::settings::list_visible_repo_uids`]), through a
    /// short-TTL in-process cache (see [`VisibleRepoUidsCache`]'s own doc
    /// comment for why that's safe here) instead of a real backend list
    /// call on every call. `gitquarry-server`'s startup sequence calls this
    /// once before binding its listeners so the very first request doesn't
    /// pay for a cold list — see `serve::run`.
    pub async fn visible_repo_uids(&self) -> Result<Vec<Uid>> {
        {
            let cached = self.visible_repo_uids_cache.inner.lock().expect("cache mutex poisoned");
            if let Some((fetched_at, uids)) = cached.as_ref()
                && fetched_at.elapsed() < VISIBLE_REPO_UIDS_TTL
            {
                return Ok(uids.clone());
            }
        }
        let uids = crate::storage::settings::list_visible_repo_uids(self.backend().as_ref()).await?;
        *self.visible_repo_uids_cache.inner.lock().expect("cache mutex poisoned") =
            Some((Instant::now(), uids.clone()));
        Ok(uids)
    }

    /// `pub`, not private: `webui::repo_settings::move_repo` (in the
    /// `gitquarry-webui` crate) also needs to build a `Remote` for an
    /// arbitrary uid (checking whether a repo-name claim it's about to
    /// reclaim is genuinely stale), the same primitive
    /// [`Self::create_repo`]'s own self-heal check uses.
    pub fn remote_for_uid(&self, uid: &Uid) -> Remote {
        Remote::new(
            self.backend(),
            format!("{}/{uid}", crate::storage::REPOS_PREFIX),
            Arc::clone(&self.cipher),
        )
    }

    /// Top-level `avatars/` prefix, sibling of [`crate::storage::REPOS_PREFIX`]
    /// rather than nested under any one repo's — an avatar belongs to a user
    /// account, not a repo.
    pub fn avatar_remote(&self) -> Remote {
        Remote::new(self.backend(), "avatars".to_string(), Arc::clone(&self.cipher))
    }

    /// Resolves an already-[`crate::validate::sanitize_path`]d display name
    /// to the repo currently claiming it, via the repo-name index — `None`
    /// if nothing does. One extra index GET beyond the old direct-prefix
    /// build this replaced, same order of cost this codebase already
    /// accepts for `auth::access::resolve_by_fingerprint` on every SSH auth.
    pub async fn resolve_repo(&self, full_name: &str) -> Result<Option<ResolvedRepo>> {
        let Some(uid) =
            crate::storage::settings::resolve_repo_name(self.backend().as_ref(), &self.cipher, full_name).await?
        else {
            return Ok(None);
        };
        Ok(Some(ResolvedRepo {
            remote: self.remote_for_uid(&uid),
            uid,
        }))
    }

    /// Mints a fresh uid and claims `full_name` for it, self-healing a name
    /// squatted by a since-deleted repo (see
    /// `storage::settings::claim_repo_name_self_healing`) — the "new repo
    /// identity" primitive shared by `/_new`, fork's destination, and SSH's
    /// implicit create-on-first-push. `None` if `full_name` is genuinely
    /// claimed by a repo that still exists, or if it names a user/group
    /// namespace: a repo at `acme/team` and a subgroup called `acme/team`
    /// would both answer to `/acme/team`, so whichever exists first wins
    /// (see `auth::access::namespace_reserved`).
    ///
    /// Only the identity: no `RepoConfig` is written, so until the caller
    /// writes one the repo has no `full_name` and renders unlinked. Go
    /// through `domain::repo::create` unless the caller builds its own
    /// config (fork, template, remote fork).
    pub async fn create_repo(&self, full_name: &str) -> Result<Option<ResolvedRepo>> {
        if crate::auth::access::namespace_reserved(self.backend().as_ref(), &self.cipher, full_name).await? {
            return Ok(None);
        }
        let uid = Uid::new();
        let claimed = crate::storage::settings::claim_repo_name_self_healing(
            self.backend().as_ref(),
            &self.cipher,
            full_name,
            &uid,
            |existing_uid| {
                let remote = self.remote_for_uid(existing_uid);
                async move { remote.exists().await }
            },
        )
        .await?;
        Ok(claimed.then(|| ResolvedRepo {
            remote: self.remote_for_uid(&uid),
            uid,
        }))
    }

    /// Where `key`'s mirror lives, rejecting anything that would escape
    /// `mirrors_root` — `key` reaches here from a repo uid, but this is the
    /// one place that turns it into a path, so the check belongs here rather
    /// than at each caller.
    fn mirror_path(&self, key: &str) -> Result<PathBuf> {
        let path = self.mirrors_root.join(key);
        if !path.starts_with(&self.mirrors_root) {
            bail!(
                "invalid mirror path: {} is outside {}",
                path.display(),
                self.mirrors_root.display()
            );
        }
        Ok(path)
    }

    /// `sync` is the one seam between this and [`Self::open_remote_mirror_at`]
    /// (otherwise identical: validate path, lock, [`crate::mirror::ensure_bare_repo`],
    /// sync, `gc::touch_last_access`) — which of the two real sync mechanisms
    /// this codebase has applies, see [`crate::mirror::SyncStrategy`].
    async fn open_mirror_common(
        &self,
        key: &str,
        sync: impl crate::mirror::SyncStrategy,
        access: Access,
    ) -> Result<(PathBuf, OwnedRwLockWriteGuard<()>)> {
        let path = self.mirror_path(key)?;
        let guard = self.locks.lock(key).await;
        crate::mirror::ensure_bare_repo(&path, &self.exe_path).await?;
        // Best-effort: without it, commits this instance signed show as
        // unsigned rather than verified. That's a worse-looking page, never
        // a broken repository, and must not stop a clone or a push.
        if let Err(e) = install_instance_allowed_signers(self, &path).await {
            warn!(error = %e, "couldn't teach the mirror this instance's signing key");
        }
        sync.sync(&path).await?;
        // Marks this mirror as just-queried so the periodic sweep
        // (`crate::mirror::gc`) doesn't evict it as stale. Best-effort: a failure
        // here would just make this mirror a slightly earlier eviction
        // candidate, not worth failing the whole request over.
        let touched = match access {
            Access::Query => crate::mirror::gc::touch_last_access(&path).await,
            // A mirror the prewarm just created has no marker at all, which
            // `gc` reads as maximally stale — the next sweep would evict
            // what startup had just paid to download. Anything already on
            // disk keeps the mtime it had, so a restart can't reset the
            // eviction order.
            Access::Warm => crate::mirror::gc::touch_last_access_if_missing(&path).await,
        };
        if let Err(e) = touched {
            warn!(error = %e, "couldn't touch mirror last-access marker");
        }
        if matches!(access, Access::Query) {
            self.note_mirror_use(key);
        }
        Ok((path, guard))
    }

    /// Records, in the bucket, that `key` was just used — the index
    /// [`crate::mirror::gc::prewarm_recent_mirrors`] reads at startup.
    ///
    /// Detached, and through the long-lived backend rather than
    /// [`Self::backend`]: the write outlives the request that triggered it,
    /// so it must not borrow that request's own cached backend, and no
    /// browse or clone should ever wait on a bucket round trip that only
    /// exists to make the *next* restart faster.
    fn note_mirror_use(&self, key: &str) {
        let backend = Arc::clone(&self.backend);
        let cipher = Arc::clone(&self.cipher);
        let uid = Uid::from(key.to_string());
        tokio::spawn(async move {
            crate::mirror::recent::record_use(backend.as_ref(), &cipher, &uid).await;
        });
    }

    /// Brings `uid`'s mirror up to date without counting as a query — what
    /// the startup prewarm calls. Same work [`Self::open_mirror`] does
    /// (lock, create, sync), minus the two things that would make a restart
    /// look like traffic: it neither refreshes an existing last-access
    /// marker nor re-records the repo as recently used.
    pub async fn warm_mirror(&self, uid: &Uid) -> Result<()> {
        let remote = self.remote_for_uid(uid);
        let sync = crate::mirror::FromBucket { remote: &remote };
        self.open_mirror_common(uid.as_str(), sync, Access::Warm).await?;
        Ok(())
    }

    /// Keyed by `resolved.uid`, not the display name — the local mirror
    /// survives a rename untouched, unlike before this uid scheme existed (a
    /// `move_repo` never touched `mirrors_root`, so a renamed repo's old
    /// mirror just sat there until `mirror::gc` aged it out; now there's
    /// nothing to orphan).
    pub async fn open_mirror(&self, resolved: &ResolvedRepo) -> Result<Mirror> {
        let sync = crate::mirror::FromBucket {
            remote: &resolved.remote,
        };
        let (path, guard) = self
            .open_mirror_common(resolved.uid.as_str(), sync, Access::Query)
            .await?;
        Ok(Mirror {
            remote: resolved.remote.clone(),
            path,
            _guard: MirrorGuard::Exclusive(guard),
        })
    }

    /// Same lock/ensure/sync/touch shape as [`Self::open_mirror`], but for a
    /// linked peer's repo instead of one in this bucket — federation layer 2
    /// (`docs/federation-concept.md`). Keyed by `_remote/<peer_label>/<repo>`
    /// (both already validated: `peer_label` via `access::validate_peer_name`
    /// at add time, `repo_name` by the caller's own
    /// `validate::sanitize_path` before this is ever called), so it can
    /// never collide with a local mirror's own uid-keyed path or lock —
    /// uids are opaque hex, never containing `/`. Synced unconditionally on
    /// every call, same as `sync_bare_repo_from_s3` — no separate
    /// staleness check, `git fetch --prune` is already a cheap no-op when
    /// nothing changed on the peer's side.
    ///
    /// Takes a bare `(peer_label, git_base_url)` pair rather than a
    /// `crate::federation::entities::Peer` — after the core/federation
    /// binary split (`docs/federation-binary-split-concept.md` section 14)
    /// core no longer keeps that type around just to call this; every
    /// caller that still has an old `Peer` value passes its two fields
    /// explicitly instead.
    pub async fn open_remote_mirror_at(
        &self,
        peer_label: &str,
        git_base_url: &str,
        repo_name: &str,
    ) -> Result<PeerMirror> {
        let rel = format!("_remote/{peer_label}/{repo_name}");
        let url = format!("{}/{repo_name}", git_base_url.trim_end_matches('/'));
        let sync = crate::mirror::FromRemoteUrl { url: &url };
        let (path, guard) = self.open_mirror_common(&rel, sync, Access::Query).await?;
        Ok(PeerMirror { path, _guard: guard })
    }

    /// [`Self::open_mirror`] for a request that only reads the mirror — a
    /// web UI page, a clone. Holds the repo's lock *shared* whenever the
    /// mirror already matches the manifest in the bucket, which is every
    /// view of a repo nobody has pushed to since the last one, so
    /// concurrent readers of the same repo stop queueing behind each other.
    ///
    /// Falls back to the exclusive path when the mirror is missing or
    /// stale, because bringing it up to date *is* a write (`git init`, pack
    /// downloads, `update-ref`). That guard is then kept for the rest of
    /// the request rather than downgraded: `RwLock` can't downgrade
    /// atomically, and re-checking freshness a second time just to widen a
    /// single already-slow render isn't worth another round trip.
    pub async fn open_mirror_read(&self, resolved: &ResolvedRepo) -> Result<Mirror> {
        let key = resolved.uid.as_str();
        let path = self.mirror_path(key)?;
        let shared = self.locks.lock_shared(key).await;
        if crate::mirror::is_synced(&resolved.remote, &path).await? {
            // Debounced, unlike the exclusive path's unconditional touch:
            // without it a repo that's browsed constantly but never pushed
            // to would age out of `mirror::gc`'s sweep and be re-downloaded
            // for no reason. Best-effort for the same reason it is there.
            if let Err(e) = crate::mirror::gc::touch_last_access_if_stale(&path).await {
                warn!(error = %e, "couldn't touch mirror last-access marker");
            }
            // Debounced the same way inside `recent::record_use`, and for
            // the same reason: this is the path most repo traffic takes.
            self.note_mirror_use(key);
            return Ok(Mirror {
                remote: resolved.remote.clone(),
                path,
                _guard: MirrorGuard::Shared(shared),
            });
        }
        drop(shared);
        self.open_mirror(resolved).await
    }
}

/// Wires [`crate::git::signing::install_allowed_signers`] up with the key
/// this bucket owns — split out of [`AppContext::open_mirror_common`] only
/// so its error can be logged and swallowed there without burying the
/// happy path in a nested match.
async fn install_instance_allowed_signers(ctx: &AppContext, bare_repo: &std::path::Path) -> Result<()> {
    // The presence check first, and only then the key: `install_allowed_signers`
    // returns early for a mirror that already has the file, but reading the
    // key to hand it one costs a bucket `get` (and, the very first time,
    // *creates* the key) — paid on every open of every already-configured
    // mirror before this check moved up here.
    if crate::git::signing::allowed_signers_installed(bare_repo).await {
        return Ok(());
    }
    let key = crate::git::signing::load_or_create(ctx.backend().as_ref(), &ctx.cipher).await?;
    crate::git::signing::install_allowed_signers(bare_repo, &key).await
}

pub fn default_mirrors_root(bucket: &str) -> PathBuf {
    let sanitized: String = bucket
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("gitquarry")
        .join("mirrors")
        .join(sanitized)
}
