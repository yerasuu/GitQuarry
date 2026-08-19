//! Local `gitquarry.toml` loading. The first-run wizard that creates one
//! when it's missing now lives in `gitquarry-server` (as `setup`), not
//! here — it needs `gitquarry-webui`'s `esc`/`layout`/`non_empty` to render
//! its form, and this crate can't depend on that without a cycle (see
//! `docs/webui-crate-split-concept.md`).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// Which storage driver a bucket uses, and that driver's connection
// parameters, now live in the `s3db` crate (it needs them to build a
// `Backend` without depending on gitquarry) — re-exported here so
// `gitquarry.toml` parsing and every existing call site keep working
// unchanged.
pub use s3db::{AzureParams, BackendConfig, GcsParams, LocalEngine, LocalParams, R2Params, S3Params};

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
}

/// `[ci]` in `gitquarry.toml` — optional, same "`None` = feature fully off"
/// internal-API endpoints are reachable and core never tries to reach a
/// `gitquarry-ci` process. See `docs/ci-cd-concept.md`.
#[derive(Debug, Clone, Deserialize)]
pub struct CiSection {
    /// Loopback address a separate `/_internal/*` server (dedicated to CI,
    /// `docs/ci-cd-concept.md`'s IPC design decision) binds to.
    pub internal_bind: String,
    /// Shared secret gating that server, manually kept identical to
    /// `gitquarry-ci.toml`'s own `internal_token`.
    pub internal_token: String,
    /// `gitquarry-ci`'s own loopback address (its `listen`) — where core
    /// would forward a pipeline-trigger push, once that call site exists.
    pub ci_url: String,
}

// pub (and `buckets` pub too) isn't just for tests/config.rs: RawConfig is
// also used unconditionally by `Config::load_from` below, so it can't be
// cfg-gated to test builds only.
#[derive(Debug, Deserialize)]
pub struct RawConfig {
    #[serde(default)]
    pub buckets: BTreeMap<String, BucketConfig>,
    #[serde(default)]
    ci: Option<CiSection>,
    /// Whether `git push` to a repo name that doesn't exist yet creates it
    /// (same as `webui::repo_new`, just reached via `git push` instead of
    /// `/_new`). Defaults to off: a typo'd repo name in the push URL should
    /// fail loudly, not silently create a new empty repo.
    #[serde(default)]
    allow_push_create: bool,
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
    ci: Option<CiSection>,
    allow_push_create: bool,
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
        Ok(Config {
            buckets: raw.buckets,
            ci: raw.ci,
            allow_push_create: raw.allow_push_create,
            path: path.to_path_buf(),
        })
    }

    /// `None` unless `[ci]` is present in `gitquarry.toml` — see
    /// [`CiSection`]'s own doc comment.
    pub fn ci(&self) -> Option<&CiSection> {
        self.ci.as_ref()
    }

    /// See [`RawConfig::allow_push_create`]'s doc comment.
    pub fn allow_push_create(&self) -> bool {
        self.allow_push_create
    }

    /// Config path that would be used per the usual resolution order. Useful
    /// for propagating it explicitly via `GITQUARRY_CONFIG` to subprocesses
    /// (e.g. git hooks) that run with a different cwd and wouldn't resolve it
    /// the same way.
    pub fn resolve_path() -> Result<PathBuf> {
        resolve_path()
    }

    /// If the config has exactly one bucket configured, its name. Lets the
    /// server start without passing `--bucket` when there's no ambiguity.
    pub fn only_bucket_name(&self) -> Option<&str> {
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
    /// section — what the first-run setup wizard ([`setup`]) calls once
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
                eprintln!(
                    "gitquarry: warning: {} has permissions {:o} and holds credentials; \
                     consider `chmod 600 {}`",
                    path.display(),
                    mode & 0o777,
                    path.display()
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
