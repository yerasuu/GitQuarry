//! A local user's own credential toward a linked [`crate::entities::Peer`] —
//! federation layer 4's other half. Distinct from [`crate::entities::AccessToken`]
//! (a credential *this* instance mints for someone else to use against
//! *it*): this is the reverse — which secret *this* user pastes in to use
//! *against* a peer instance B, where B minted it and holds the only
//! record of what it's scoped to. Opaque here, just stored for outbound
//! calls against B to attach as a `Bearer` credential.
//!
//! `.conf/peer-tokens/<user_uid>/<peer_name>.json` — a per-user-scoped
//! [`Collection`], built with [`Collection::new_owned`] rather than
//! `#[derive(Entity)]`, since the derive's `#[entity(collection = "...")]`
//! needs a compile-time string literal, which can't embed a per-user uid
//! computed at runtime. Genuinely sensitive data, not a hint/cache — lives
//! under `.conf/peer-tokens/`, not `.conf/index/` — but no extra crypto of
//! its own is needed beyond what every object already gets: `s3db`
//! encrypts everything uploaded unconditionally.
//!
//! Also reused, unchanged, for the sister-instance handshake secret
//! exchange (`docs/federation-binary-split-concept.md` section 7) — same
//! "an opaque secret linking this instance to another one" shape, no need
//! for a second mechanism.

use crate::time::{format_unix_secs, now_secs};
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::{Backend, Cipher, Collection};
use serde::{Deserialize, Serialize};

const MAX_SECRET_LEN: usize = 512;
const MAX_PEER_LABEL_LEN: usize = 128;

/// One linked credential: the opaque secret B minted, plus a display-only
/// label — no `expires_at`/`repos`/`write` here, since B is the only party
/// that knows or enforces any of that; A just holds whatever secret it was
/// given.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PeerCredential {
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub linked_at: String,
}

/// Validates a credential's secret/label before storing — not a format
/// check: the secret is opaque, minted by another instance entirely, so
/// there's nothing about its shape to validate beyond "not empty, not
/// unbounded".
pub fn validate_peer_credential(secret: &str, label: &str) -> Result<()> {
    if secret.trim().is_empty() || secret.len() > MAX_SECRET_LEN {
        bail!("empty or too-long secret (max {MAX_SECRET_LEN})");
    }
    if label.len() > MAX_PEER_LABEL_LEN {
        bail!("too-long label (max {MAX_PEER_LABEL_LEN})");
    }
    if secret.contains(|c: char| c.is_control()) {
        bail!("invalid secret (control characters)");
    }
    Ok(())
}

fn peer_tokens_collection(user_uid: &Uid) -> Collection {
    Collection::new_owned(format!(".conf/peer-tokens/{user_uid}"))
}

/// Upsert, not create-only: unlike [`crate::entities::AccessToken`] (minted
/// once, secret never re-enterable), a peer credential is exactly the kind
/// of thing a user re-pastes after rotating it on B's side — always
/// overwrites whatever was there for this `(user_uid, peer_name)` pair.
pub async fn set_peer_credential(
    backend: &dyn Backend,
    cipher: &Cipher,
    user_uid: &Uid,
    peer_name: &str,
    secret: &str,
    label: &str,
) -> Result<()> {
    let (_existing, etag) = peer_tokens_collection(user_uid)
        .get::<PeerCredential>(backend, cipher, peer_name)
        .await?;
    let credential = PeerCredential {
        secret: secret.to_string(),
        label: label.to_string(),
        linked_at: format_unix_secs(now_secs()),
    };
    peer_tokens_collection(user_uid)
        .put(backend, cipher, peer_name, &credential, etag.as_deref())
        .await?;
    Ok(())
}

pub async fn get_peer_credential(
    backend: &dyn Backend,
    cipher: &Cipher,
    user_uid: &Uid,
    peer_name: &str,
) -> Result<Option<PeerCredential>> {
    Ok(peer_tokens_collection(user_uid)
        .get::<PeerCredential>(backend, cipher, peer_name)
        .await?
        .0)
}

/// Every peer this `user_uid` has a credential linked for, sorted by peer
/// name — small by construction.
pub async fn list_peer_credentials(
    backend: &dyn Backend,
    cipher: &Cipher,
    user_uid: &Uid,
) -> Result<Vec<(String, PeerCredential)>> {
    let names = peer_tokens_collection(user_uid).list_names(backend).await?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        if let Some(credential) = get_peer_credential(backend, cipher, user_uid, &name).await? {
            out.push((name, credential));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub async fn delete_peer_credential(backend: &dyn Backend, cipher: &Cipher, user_uid: &Uid, peer_name: &str) {
    let _ = peer_tokens_collection(user_uid)
        .delete(backend, cipher, peer_name)
        .await;
}
