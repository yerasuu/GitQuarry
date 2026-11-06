//! Hand-rolled, dependency-free validation for the handful of free-text
//! fields this crate stores — same charset-and-prefix style as
//! `gitquarry-core`'s own `validate.rs` (not shared: this crate never links
//! against core, by design — see the workspace split doc).

use anyhow::{Result, bail};

const MAX_INSTANCE_URL_LEN: usize = 512;

/// A peer's or gossip candidate's `web_base_url` — dialed over HTTP for
/// `/_api/*` and `/_federation/*`, so only an HTTP scheme makes sense.
pub fn validate_web_base_url(url: &str) -> Result<()> {
    validate_url(url, &["https://", "http://"])
}

/// A peer's or gossip candidate's `git_base_url` — handed to core's
/// `git fetch` for federation layer 2. `git://` is refused because no
/// gitquarry instance serves it any more (the daemon was dropped), so a
/// peer advertising one is either stale or not a gitquarry instance.
pub fn validate_git_base_url(url: &str) -> Result<()> {
    validate_url(url, &["https://", "http://", "ssh://"])
}

fn validate_url(url: &str, schemes: &[&str]) -> Result<()> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_INSTANCE_URL_LEN {
        bail!("empty or too-long instance URL (max {MAX_INSTANCE_URL_LEN})");
    }
    let rest = schemes
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
        .ok_or_else(|| anyhow::anyhow!("invalid instance URL (expected {}): {url}", schemes.join(" or ")))?;
    if rest.is_empty() || rest.starts_with('/') {
        bail!("invalid instance URL (missing host): {url}");
    }
    if url.contains(|c: char| c.is_control() || c.is_whitespace()) {
        bail!("invalid instance URL (disallowed characters): {url}");
    }
    Ok(())
}
