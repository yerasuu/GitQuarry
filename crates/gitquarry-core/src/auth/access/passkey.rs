//! [`Passkey`] storage: the credential records themselves plus the
//! owner-scoped listing index, same shape as `access::pat`'s.

use crate::auth::entities::Passkey;
use crate::storage::INDEX_PREFIX;
use crate::storage::uid::Uid;
use anyhow::{Result, bail};
use s3db::{Backend, Cipher, Collection};
use sha2::{Digest, Sha256};

const MAX_PASSKEY_LABEL_LEN: usize = 64;
/// Per user. Bounds the listing and the `allowCredentials` a second-factor
/// challenge carries; nobody has more real authenticators than this.
pub const MAX_PASSKEYS_PER_USER: usize = 16;

/// Validates a passkey's display label — owner-chosen free text ending up
/// in an HTML page, never a key segment.
pub fn validate_passkey_label(label: &str) -> Result<()> {
    if label.trim().is_empty() || label.len() > MAX_PASSKEY_LABEL_LEN {
        bail!("empty or too-long passkey name (max {MAX_PASSKEY_LABEL_LEN})");
    }
    if label.contains(|c: char| c.is_control()) {
        bail!("invalid passkey name (control characters)");
    }
    Ok(())
}

/// The storage key for a credential id (its base64url form): SHA-256 hex,
/// fixed-length and key-safe whatever the authenticator chose.
pub fn passkey_key(credential_id: &str) -> String {
    use std::fmt::Write;
    Sha256::digest(credential_id.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn owner_index(owner_uid: &Uid) -> Collection {
    Collection::new_owned(format!("{INDEX_PREFIX}/passkey-owner/{owner_uid}"))
}

/// Best-effort: a missed write only hides a passkey from its owner's
/// listing (and from a second-factor challenge's `allowCredentials`), never
/// lets anyone else use it — [`find_by_credential_id`] never consults it.
pub(crate) async fn add_passkey_owner_index(backend: &dyn Backend, cipher: &Cipher, owner_uid: &Uid, key: &str) {
    let _ = owner_index(owner_uid).put(backend, cipher, key, &(), None).await;
}

pub(crate) async fn remove_passkey_owner_index(backend: &dyn Backend, cipher: &Cipher, owner_uid: &Uid, key: &str) {
    let _ = owner_index(owner_uid).delete(backend, cipher, key).await;
}

/// Every passkey `owner_uid` registered, oldest first. A record whose owner
/// no longer matches (a stale index entry) is left out.
pub async fn list_passkeys_for_owner(backend: &dyn Backend, cipher: &Cipher, owner_uid: &Uid) -> Result<Vec<Passkey>> {
    let keys = owner_index(owner_uid).list_names(backend).await?;
    let mut passkeys = Vec::with_capacity(keys.len());
    for key in keys {
        if let (Some(passkey), _) = Passkey::get(backend, cipher, &key).await?
            && &passkey.owner_uid == owner_uid
        {
            passkeys.push(passkey);
        }
    }
    passkeys.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(passkeys)
}

/// The passkey a sign-in names by its credential id, and its etag for the
/// counter update. Re-checks the stored id against the one asked for, so
/// only an exact match is ever returned.
pub async fn find_by_credential_id(
    backend: &dyn Backend,
    cipher: &Cipher,
    credential_id: &str,
) -> Result<Option<(Passkey, Option<String>)>> {
    let (passkey, etag) = Passkey::get(backend, cipher, &passkey_key(credential_id)).await?;
    Ok(passkey.filter(|p| p.credential_id == credential_id).map(|p| (p, etag)))
}
