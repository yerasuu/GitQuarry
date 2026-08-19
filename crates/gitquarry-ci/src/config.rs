//! `gitquarry-ci.toml` loading — a separate file from `gitquarry-core`'s own
//! `gitquarry.toml`, same env>cwd>gitroot>userglobal resolution order,
//! reimplemented locally against `s3db::BackendConfig` rather than depending
//! on `gitquarry-core`'s config module (this crate never depends on that
//! crate at all — same "talks to core only over loopback HTTP" posture
//! `gitquarry-federation` already established).
//!
//! `bucket`/`passphrase` (this instance's own storage, bootstrapped against
//! the *same* bucket `gitquarry-core` uses — different code, same
//! passphrase/namespace), `listen` (this instance's own loopback bind, what
//! core's `[ci]` section calls `ci_url`), `core_internal_url`/`internal_token`
//! (reaching core's `/_internal/*`, specifically the `ci`-scoped instance of
//! it — see `docs/ci-cd-concept.md`'s IPC design decision).

use anyhow::{Context, Result, bail};
use s3db::BackendConfig;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[cfg(any(test, feature = "test-util"))]
pub const CONFIG_NAME: &str = "gitquarry-ci.toml";
#[cfg(not(any(test, feature = "test-util")))]
const CONFIG_NAME: &str = "gitquarry-ci.toml";

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(flatten)]
    pub backend: BackendConfig,
    pub passphrase: String,
    pub listen: String,
    pub core_internal_url: String,
    pub internal_token: String,
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "couldn't read config {} (see gitquarry-ci.toml.example)",
                path.display()
            )
        })?;
        let config: Config = toml::from_str(&text).with_context(|| format!("invalid config in {}", path.display()))?;
        if config.passphrase.trim().is_empty() {
            bail!(
                "{} has no `passphrase` (mandatory: must match gitquarry-core's own bucket passphrase)",
                path.display()
            );
        }
        if config.internal_token.trim().is_empty() {
            bail!(
                "{} has no `internal_token` (mandatory: must match gitquarry-core's [ci].internal_token)",
                path.display()
            );
        }
        Ok(config)
    }

    /// Same resolution order as `gitquarry-core`'s own config: explicit env
    /// var > cwd > git repo root > user global.
    pub fn resolve_path() -> Result<PathBuf> {
        if let Some(from_env) = std::env::var_os("GITQUARRY_CI_CONFIG") {
            return Ok(absolutize(PathBuf::from(from_env)));
        }
        if let Some(local) = find_local_config() {
            return Ok(local);
        }
        let base = dirs::config_dir().context("couldn't find the system config directory")?;
        Ok(base.join("gitquarry").join(CONFIG_NAME))
    }
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
    let cwd_candidate = PathBuf::from(CONFIG_NAME);
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
    let candidate = PathBuf::from(root).join(CONFIG_NAME);
    candidate.is_file().then_some(candidate)
}
