//! [`Peer`] CRUD and listing — federation layer 1. Deliberately thinner
//! than [`super::tokens`]: no secondary index, since nothing ever needs to
//! resolve a `Peer` by anything other than its own `name`.

use crate::entities::Peer;
use crate::validate::validate_instance_url;
use anyhow::{Result, bail};
use s3db::{Backend, Cas, Cipher};

const MAX_PEER_NAME_LEN: usize = 128;
const MAX_PEER_URL_LEN: usize = 512;

/// Validates a peer's display name before it's stored — it's also a
/// literal S3 key segment (`.conf/peers/<name>.json`).
pub fn validate_peer_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > MAX_PEER_NAME_LEN {
        bail!("empty or too-long peer name (max {MAX_PEER_NAME_LEN})");
    }
    if name.contains(|c: char| c.is_control()) {
        bail!("invalid peer name (control characters)");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("invalid peer name (can't contain '/' or '\\\\')");
    }
    Ok(())
}

/// Validates a peer's `web_base_url`/`git_base_url`.
pub fn validate_peer_url(url: &str) -> Result<()> {
    if url.len() > MAX_PEER_URL_LEN {
        bail!("too-long peer URL (max {MAX_PEER_URL_LEN})");
    }
    validate_instance_url(url)
}

/// Fetches one peer by name — `None` means "no such peer".
pub async fn get_peer(backend: &dyn Backend, cipher: &Cipher, name: &str) -> Result<Option<Peer>> {
    Ok(Peer::get(backend, cipher, name).await?.0)
}

/// CAS-writes one peer's full record to its own key.
pub async fn put_peer(backend: &dyn Backend, cipher: &Cipher, peer: &Peer, expected_etag: Option<&str>) -> Result<Cas> {
    peer.put(backend, cipher, expected_etag).await
}

/// Best-effort delete of a peer's own object.
pub async fn delete_peer_object(backend: &dyn Backend, cipher: &Cipher, name: &str) {
    Peer::delete(backend, cipher, name).await;
}

/// Every `Peer`, via the collection's own key listing then a concurrent
/// fan-out of tiny per-peer GETs. The peer count is admin/moderator-curated
/// and expected to stay small, so this is fine to call on every `/_peers`
/// render without a cache of its own.
pub async fn list_all_peers(backend: &dyn Backend, cipher: &Cipher) -> Result<Vec<Peer>> {
    let names = Peer::list_keys(backend).await?;
    let mut peers = Vec::with_capacity(names.len());
    for name in names {
        if let (Some(peer), _) = Peer::get(backend, cipher, &name).await? {
            peers.push(peer);
        }
    }
    peers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(peers)
}
