//! `gitquarry-federation.toml` loading — a separate file from
//! `gitquarry-core`'s own `gitquarry.toml`, same env>cwd>gitroot>userglobal
//! resolution order, reimplemented locally against `s3db::BackendConfig`
//! rather than depending on `gitquarry-core`'s config module (this crate
//! never depends on that crate at all — see the workspace split doc).
//!
//! `bucket`/`passphrase` (this instance's own storage), `listen`
//! (federation's loopback bind), `core_internal_url`/`internal_token`
//! (reaching core's `/_internal/*`), plus (section 5, gossip discovery)
//! `seed_peers`/`gossip_interval_secs`/`auto_link_min_sources`/
//! `auto_link_min_age_secs` and the two `own_*_base_url` fields a gossip
//! announcement needs to tell another instance how to reach *this* one —
//! not in the doc's original sketch config, added here because nothing
//! else in this config identifies this instance's own public address.
//! `rate_limit_public_*`/`rate_limit_token_*` (section 6) size the two
//! independent buckets `rate_limit::PeerRateLimiter` enforces;
//! `rate_limit_peer_override_by_ip` toggles whether `Peer::rate_limit_override`
//! applies at all (see its own doc comment on the tradeoff).

use anyhow::{Context, Result, bail};
use s3db::BackendConfig;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const CONFIG_NAME: &str = "gitquarry-federation.toml";

fn default_gossip_interval_secs() -> u64 {
    300
}

fn default_auto_link_min_sources() -> u32 {
    3
}

fn default_auto_link_min_age_secs() -> u64 {
    24 * 60 * 60
}

fn default_rate_limit_public_max() -> u32 {
    60
}

fn default_rate_limit_window_secs() -> u64 {
    60
}

fn default_rate_limit_token_max() -> u32 {
    120
}

fn default_rate_limit_peer_override_by_ip() -> bool {
    true
}

fn default_sister_reconcile_interval_secs() -> u64 {
    300
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(flatten)]
    pub backend: BackendConfig,
    pub passphrase: String,
    pub listen: String,
    pub core_internal_url: String,
    pub internal_token: String,
    /// `https://<this instance's own public host>` — told to other
    /// instances during gossip so they know how to reach *this* one back.
    /// `None` (the default) means gossip bootstrap/announce-handling is
    /// disabled even if `seed_peers` is set: there's no meaningful
    /// candidate to tell anyone about otherwise.
    #[serde(default)]
    pub own_web_base_url: Option<String>,
    #[serde(default)]
    pub own_git_base_url: Option<String>,
    /// Bootstrap contacts for gossip discovery (section 5) — reached
    /// through *their* own core's public proxy, same as any other peer
    /// call, never loopback. Empty (the default) means no outbound gossip
    /// tick runs at all, though this instance still answers incoming
    /// `/_federation/gossip/announce` from others.
    #[serde(default)]
    pub seed_peers: Vec<String>,
    #[serde(default = "default_gossip_interval_secs")]
    pub gossip_interval_secs: u64,
    #[serde(default = "default_auto_link_min_sources")]
    pub auto_link_min_sources: u32,
    #[serde(default = "default_auto_link_min_age_secs")]
    pub auto_link_min_age_secs: u64,
    /// Requests per window for `/_api/*`/`/_federation/gossip/*` callers
    /// with no (or an invalid) bearer token, keyed by the caller's IP —
    /// see `rate_limit`'s own module doc comment for where that IP
    /// actually comes from through the proxy.
    #[serde(default = "default_rate_limit_public_max")]
    pub rate_limit_public_max: u32,
    #[serde(default = "default_rate_limit_window_secs")]
    pub rate_limit_public_window_secs: u64,
    /// Requests per window for a caller with a valid bearer token, keyed
    /// by the resolved `AccessToken.owner`.
    #[serde(default = "default_rate_limit_token_max")]
    pub rate_limit_token_max: u32,
    #[serde(default = "default_rate_limit_window_secs")]
    pub rate_limit_token_window_secs: u64,
    /// Whether a linked peer's own `rate_limit_override` applies at all to
    /// the public bucket — see `web::rate_limit_middleware::peer_override_for_ip`'s
    /// own doc comment on why this is a caller-IP-vs-stored-host text
    /// match, never a cryptographic guarantee: anyone sharing a throttled/
    /// boosted peer's egress IP gets the same treatment, and that peer can
    /// dodge its own override entirely by calling from a different IP.
    /// Defaults to `true` (keep the heuristic on) — the doc's own closing
    /// section raised this as an open question and this is the answer:
    /// the override is opt-in and moderator-set to begin with (nothing
    /// changes for an instance that never sets one), so the imprecision
    /// only ever affects a peer someone already chose to single out, never
    /// a peer nobody touched. An operator who doesn't want that
    /// collateral-attribution risk at all — even for peers they've
    /// explicitly overridden — can set this to `false` and every public
    /// caller just gets the instance-wide ceiling, no exceptions.
    #[serde(default = "default_rate_limit_peer_override_by_ip")]
    pub rate_limit_peer_override_by_ip: bool,
    /// How often the periodic sister ban-list reconciliation
    /// (`sister::reconcile_with_sister`, section 7 step 2) runs against
    /// every `sister_confirmed` peer — same 300s default as
    /// `gossip_interval_secs`, since both exist to catch what a lost
    /// push/announce missed, not to be a primary delivery path.
    #[serde(default = "default_sister_reconcile_interval_secs")]
    pub sister_reconcile_interval_secs: u64,
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "couldn't read config {} (see gitquarry-federation.toml.example)",
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
                "{} has no `internal_token` (mandatory: must match gitquarry-core's [federation].internal_token)",
                path.display()
            );
        }
        Ok(config)
    }

    /// Same resolution order as `gitquarry-core`'s own config: explicit env
    /// var > cwd > git repo root > user global.
    pub fn resolve_path() -> Result<PathBuf> {
        if let Some(from_env) = std::env::var_os("GITQUARRY_FEDERATION_CONFIG") {
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
