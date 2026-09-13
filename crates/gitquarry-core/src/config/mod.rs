//! Local `gitquarry.toml` loading. The first-run wizard that creates one
//! when it's missing now lives in `gitquarry-server` (as `setup`), not
//! here — it needs `gitquarry-webui`'s `esc`/`layout`/`non_empty` to render
//! its form, and this crate can't depend on that without a cycle (see
//! `docs/webui-crate-split-concept.md`).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::warn;

// Which storage driver a bucket uses, and that driver's connection
// parameters, now live in the `s3db` crate (it needs them to build a
// `Backend` without depending on gitquarry) — re-exported here so
// `gitquarry.toml` parsing and every existing call site keep working
// unchanged.
pub use s3db::{AzureParams, BackendConfig, GcsParams, LocalEngine, LocalParams, R2Params, ResilienceConfig, S3Params};

/// The main toml (buckets/credentials/passphrase) lives loose at the project
/// root, NOT under `.conf/`: `.conf/` is for everything else (remote config
/// on the bucket side in the storage backend, see
/// `storage::settings`/`auth::access`/`crypto::bootstrap` — a different
/// namespace, backend keys, not local filesystem).
const LOCAL_CONFIG_NAME: &str = "gitquarry.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketConfig {
    #[serde(flatten)]
    pub backend: BackendConfig,
    /// Passphrase that encrypts everything this bucket uploads (packs,
    /// refs, settings, SSH keys — everything except object names). Required
    /// regardless of driver: the storage provider is an intermediary, it
    /// must never be able to read anything in the clear. Never stored on
    /// the backend; only the salt derived from it (see
    /// [`crate::crypto::bootstrap`]).
    pub passphrase: String,
    /// Concurrency caps, retry/backoff, circuit breaker, and the
    /// cross-request raw-bytes LRU (`resilience.cache.*`) that
    /// [`crate::storage::build_backend`] wraps this bucket's driver in.
    /// Missing entirely from `gitquarry.toml` (or any field within it) falls
    /// back to `s3db`'s own defaults — a 10k-entry/30s-TTL cache and
    /// 64/32/16 concurrent get/put/list.
    #[serde(default)]
    pub resilience: ResilienceConfig,
}

/// `[tracker]` in `gitquarry.toml` — optional, same "`None` = feature fully
/// off, never a half-wired no-op" pattern `oidc: Option<OidcClient>`
/// already establishes. Absent means core has no way to reach `gitquarry-tracker`'s
/// loopback API and the issues/board/milestones tabs disappear entirely
/// from the web UI. See `docs/tracker-binary-split-concept.md`.
#[derive(Debug, Clone, Deserialize)]
pub struct TrackerSection {
    /// Loopback address the `/_internal/*` server binds to — see
    /// `docs/tracker-binary-split-concept.md`.
    pub internal_bind: String,
    /// Shared secret gating that server, manually kept identical to
    /// `gitquarry-tracker.toml`'s own `internal_token`.
    pub internal_token: String,
    /// `gitquarry-tracker`'s own loopback address (its `listen`) — where
    /// `webui` forwards issue/board/milestone CRUD, and where core pushes a
    /// PR-merge notification for issue auto-close.
    pub tracker_url: String,
}

// pub (and `buckets` pub too) isn't just for tests/config.rs: RawConfig is
// also used unconditionally by `Config::load_from` below, so it can't be
// cfg-gated to test builds only.
#[derive(Debug, Deserialize)]
pub struct RawConfig {
    #[serde(default)]
    pub buckets: BTreeMap<String, BucketConfig>,
    #[serde(default)]
    tracker: Option<TrackerSection>,
    /// Which bucket this instance serves when no `--bucket` is passed. Only
    /// meaningful with several sections configured, which is the normal
    /// shape once a bucket exists purely as a backup destination
    /// (`backup::dest::Dest::Bucket`) or a mirror target: those are never
    /// served, so leaving the choice to "there is only one" stopped being
    /// enough. Must name a configured bucket — a typo here would otherwise
    /// only surface at startup as "bucket not configured".
    #[serde(default)]
    pub default_bucket: Option<String>,
    /// Whether `git push` to a repo name that doesn't exist yet creates it
    /// (same as `webui::repo_new`, just reached via `git push` instead of
    /// `/_new`). Defaults to off: a typo'd repo name in the push URL should
    /// fail loudly, not silently create a new empty repo.
    #[serde(default)]
    allow_push_create: bool,
    /// Whether `X-Forwarded-For`'s first hop may be taken as the client
    /// address for anonymous git rate limiting. Off by default, and it has
    /// to stay off unless a reverse proxy in front of this server always
    /// *overwrites* that header: a client that can set it freely rotates
    /// its own rate-limit bucket at will, which is strictly worse than the
    /// socket address. On, behind such a proxy, it's the only way the limit
    /// distinguishes clients at all — every request otherwise arrives from
    /// the proxy's single address and shares one bucket.
    #[serde(default)]
    trust_forwarded_for: bool,
}

/// Mirror of `RawConfig` for writing instead of reading — kept separate so
/// the `Deserialize` (tolerant, e.g. `#[serde(default)]`) and `Serialize`
/// (exact, always emits `buckets`) shapes don't have to compromise for each
/// other, and so the reading path never accidentally gains a `Serialize`
/// bound it doesn't need.
#[derive(Serialize)]
struct RawConfigOut<'a> {
    buckets: BTreeMap<&'a str, &'a BucketConfig>,
}

#[derive(Debug)]
pub struct Config {
    buckets: BTreeMap<String, BucketConfig>,
    tracker: Option<TrackerSection>,
    default_bucket: Option<String>,
    allow_push_create: bool,
    trust_forwarded_for: bool,
    /// Where it was loaded from. Goes into error messages so you can fix it
    /// without guessing which candidate won.
    path: PathBuf,
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "couldn't read config {} (create the file with [buckets.<name>] sections)",
                path.display()
            )
        })?;
        let raw: RawConfig = toml::from_str(&text).with_context(|| format!("invalid config in {}", path.display()))?;
        if raw.buckets.is_empty() {
            bail!(
                "config {} has no buckets: add a [buckets.<name>] section",
                path.display()
            );
        }
        for (name, bucket) in &raw.buckets {
            if bucket.passphrase.trim().is_empty() {
                bail!(
                    "[buckets.{name}] in {} has no `passphrase` (mandatory: encrypts everything \
                     uploaded to this bucket)",
                    path.display()
                );
            }
        }
        if let Some(name) = &raw.default_bucket
            && !raw.buckets.contains_key(name)
        {
            bail!(
                "default_bucket = \"{name}\" in {} names no configured bucket (add [buckets.{name}] \
                 or fix the name)",
                path.display()
            );
        }
        Ok(Config {
            buckets: raw.buckets,
            tracker: raw.tracker,
            default_bucket: raw.default_bucket,
            allow_push_create: raw.allow_push_create,
            trust_forwarded_for: raw.trust_forwarded_for,
            path: path.to_path_buf(),
        })
    }

    /// `None` unless `[tracker]` is present in `gitquarry.toml` — see
    /// [`TrackerSection`]'s own doc comment.
    pub fn tracker(&self) -> Option<&TrackerSection> {
        self.tracker.as_ref()
    }

    /// See [`RawConfig::allow_push_create`]'s doc comment.
    pub fn allow_push_create(&self) -> bool {
        self.allow_push_create
    }

    /// See [`RawConfig::trust_forwarded_for`]'s doc comment.
    pub fn trust_forwarded_for(&self) -> bool {
        self.trust_forwarded_for
    }

    /// Config path that would be used per the usual resolution order. Useful
    /// for propagating it explicitly via `GITQUARRY_CONFIG` to subprocesses
    /// (e.g. git hooks) that run with a different cwd and wouldn't resolve it
    /// the same way.
    pub fn resolve_path() -> Result<PathBuf> {
        resolve_path()
    }

    /// Which bucket to work on when no `--bucket` was given: whatever
    /// `default_bucket` names, or the single configured bucket when there's
    /// only one and so no ambiguity. `None` with several buckets and no
    /// `default_bucket` — the caller turns that into the error telling the
    /// operator to pick.
    pub fn default_bucket_name(&self) -> Option<&str> {
        if let Some(name) = &self.default_bucket {
            return Some(name.as_str());
        }
        match self.buckets.len() {
            1 => self.buckets.keys().next().map(String::as_str),
            _ => None,
        }
    }

    pub fn bucket(&self, name: &str) -> Result<&BucketConfig> {
        self.buckets.get(name).with_context(|| {
            format!(
                "bucket '{name}' not configured. Add the [buckets.{name}] section in {}",
                self.path.display()
            )
        })
    }

    /// Writes a brand-new `gitquarry.toml` with a single `[buckets.<name>]`
    /// section — what the first-run setup wizard (`gitquarry-server`'s `setup`) calls once
    /// it already has a bucket config it verified works against S3. Refuses
    /// to touch `path` if anything is there already: this only ever creates
    /// a config from nothing, never edits or replaces an existing one (that
    /// stays a manual edit, or a future `/_settings`-style flow, not this).
    pub fn write_new(path: &Path, bucket_name: &str, bucket_cfg: &BucketConfig) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating directory {}", parent.display()))?;
        }
        let mut buckets = BTreeMap::new();
        buckets.insert(bucket_name, bucket_cfg);
        let text = toml::to_string_pretty(&RawConfigOut { buckets }).context("serializing gitquarry.toml")?;
        write_new_private_file(path, &text)
    }
}

/// Creates `path` with permissions readable only by the owner from the
/// start (unlike `warn_if_world_readable`, which just warns after the fact
/// for a config an operator wrote by hand) — this file holds S3 credentials
/// and the bucket passphrase. `create_new` doubles as a TOCTOU guard: fails
/// instead of overwriting if something raced us and created `path` first.
fn write_new_private_file(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The config stores credentials in plain text. If anyone on the system can
/// read it, we warn: we don't abort because it might be intentional (a
/// single-user container), but total silence would be worse.
///
/// Only called by server startup, not `load_from`: the remote helper and the
/// hook run one process per git command and would repeat the warning on
/// every push.
pub fn warn_if_world_readable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                warn!(
                    path = %path.display(),
                    mode = %format_args!("{:o}", mode & 0o777),
                    "config file holds credentials but is not private; consider `chmod 600`"
                );
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Resolution order: explicit env var > `gitquarry.toml` in cwd >
/// `gitquarry.toml` at the git repo root (`git rev-parse --show-toplevel`)
/// > user global config (`~/.config/gitquarry/config.toml`).
fn resolve_path() -> Result<PathBuf> {
    // Always absolute: whoever resolves it (e.g. the server) usually passes
    // it to subprocesses (git hooks) that run with a different cwd than the
    // current one.
    if let Some(from_env) = std::env::var_os("GITQUARRY_CONFIG") {
        return Ok(absolutize(PathBuf::from(from_env)));
    }
    if let Some(local) = find_local_config() {
        return Ok(local);
    }
    let base = dirs::config_dir().context("couldn't find the system config directory")?;
    Ok(base.join("gitquarry").join("config.toml"))
}

fn absolutize(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path,
    }
}

fn find_local_config() -> Option<PathBuf> {
    let cwd_candidate = PathBuf::from(LOCAL_CONFIG_NAME);
    if cwd_candidate.is_file() {
        return Some(absolutize(cwd_candidate));
    }

    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(root).join(LOCAL_CONFIG_NAME);
    candidate.is_file().then_some(candidate)
}
