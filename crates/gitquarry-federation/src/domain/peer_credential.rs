//! Every mutation of a [`crate::access::PeerCredential`] goes through here —
//! federation layer 4. Same `domain`/`web` boundary [`crate::domain::peer`]'s
//! doc comment describes.

use crate::access;
use crate::access::PeerCredential;
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::{Backend, Cipher};

/// `Err` if `peer_name` isn't a peer this instance actually has linked —
/// pasting a credential for a made-up or no-longer-linked peer name would
/// just be dead data nothing ever reads.
pub async fn set(
    backend: &dyn Backend,
    cipher: &Cipher,
    user_uid: &Uid,
    peer_name: &str,
    secret: &str,
    label: &str,
) -> Result<()> {
    access::validate_peer_credential(secret, label)?;
    if access::get_peer(backend, cipher, peer_name).await?.is_none() {
        bail!("no such linked peer: {peer_name}");
    }
    access::set_peer_credential(backend, cipher, user_uid, peer_name, secret, label).await
}

pub async fn list(backend: &dyn Backend, cipher: &Cipher, user_uid: &Uid) -> Result<Vec<(String, PeerCredential)>> {
    access::list_peer_credentials(backend, cipher, user_uid).await
}

/// The one credential a PR-proxy call against `peer_name` needs to act as
/// `user_uid`. `None` if this user never linked one for this peer.
pub async fn get(
    backend: &dyn Backend,
    cipher: &Cipher,
    user_uid: &Uid,
    peer_name: &str,
) -> Result<Option<PeerCredential>> {
    access::get_peer_credential(backend, cipher, user_uid, peer_name).await
}

pub async fn delete(backend: &dyn Backend, cipher: &Cipher, user_uid: &Uid, peer_name: &str) {
    access::delete_peer_credential(backend, cipher, user_uid, peer_name).await;
}
