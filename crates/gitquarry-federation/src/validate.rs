//! Hand-rolled, dependency-free validation for the handful of free-text
//! fields this crate stores — same charset-and-prefix style as
//! `gitquarry-core`'s own `validate.rs` (not shared: this crate never links
//! against core, by design — see the workspace split doc).

use anyhow::{Result, bail};

const MAX_INSTANCE_URL_LEN: usize = 512;

/// A peer's `web_base_url`/`git_base_url`, or a gossip candidate's — same
/// scheme/host/no-control-chars check `gitquarry-core`'s
/// `validate::validate_instance_url` used before the split.
pub fn validate_instance_url(url: &str) -> Result<()> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_INSTANCE_URL_LEN {
        bail!("empty or too-long instance URL (max {MAX_INSTANCE_URL_LEN})");
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("git://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .ok_or_else(|| anyhow::anyhow!("invalid instance URL (expected http(s)/git/ssh scheme): {url}"))?;
    if rest.is_empty() || rest.starts_with('/') {
        bail!("invalid instance URL (missing host): {url}");
    }
    if url.contains(|c: char| c.is_control() || c.is_whitespace()) {
        bail!("invalid instance URL (disallowed characters): {url}");
    }
    Ok(())
}
