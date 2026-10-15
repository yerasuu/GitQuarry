//! SSH pubkey fingerprint index, plus fingerprint computation itself.

use super::users::get_user;
use crate::auth::entities::User;
use crate::crypto::Cipher;
use crate::storage::{Backend, INDEX_PREFIX};
use anyhow::{Context, Result};
use russh::keys::{HashAlg, PublicKey};
use s3db::{SecondaryIndex, Segment};
use serde::{Deserialize, Serialize};

/// URL-safe-base64 of `raw`, used as the S3 key segment for the pubkey
/// index below. Not a hash: reversible, but nothing here treats this key as
/// secret — the same fingerprint is visible in the plaintext body of
/// whichever `User` actually names it — just a safe encoding. Fingerprints
/// (`"SHA256:<base64>"`) contain `/`/`+`, unsafe as a literal key segment.
///
/// `pub(crate)`, not private: `auth::domain::user::create` builds its own
/// `s3db::txn::TxnOp` directly against this key, so the pubkey claim lands
/// in the same create-only transaction as the user object itself. Stays
/// hand-rolled rather than a `#[derive(Entity)]` index: a fingerprint is
/// derived from whichever of `User::pubkeys`' several entries matched, not
/// a single stored field the derive's one-field-one-index model can express.
pub(crate) const PUBKEY_INDEX: SecondaryIndex = SecondaryIndex::new(INDEX_PREFIX, "pubkey", Segment::Base64);

/// One pubkey-fingerprint index entry: which username claims it.
/// `pub(crate)`, same reasoning as [`PUBKEY_INDEX`]: `auth::domain::user::create`
/// constructs this directly to build a `TxnOp`.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsernamePointer {
    #[serde(default)]
    pub(crate) username: String,
}

/// Resolves an SSH pubkey fingerprint to its `User`, or `None` if nothing
/// genuinely claims it. This is the fast path [`crate::git::ssh`]'s
/// `auth_publickey` uses instead of scanning every user.
///
/// The index is a hint, not a fact: after following it to a candidate
/// username, this re-verifies the fingerprint is genuinely present in that
/// user's own `pubkeys` before returning them — the same verification
/// makes key revocation fail-closed even if an index-cleanup delete lagged
/// or never ran (see [`release_pubkey`]), and it's not wasted work, the
/// caller needs the full `User` anyway.
pub async fn resolve_by_fingerprint(backend: &dyn Backend, cipher: &Cipher, fingerprint: &str) -> Result<Option<User>> {
    let (pointer, _etag) = PUBKEY_INDEX
        .resolve::<UsernamePointer>(backend, cipher, fingerprint)
        .await?;
    let Some(pointer) = pointer else { return Ok(None) };
    let (user, _etag) = get_user(backend, cipher, &pointer.username).await?;
    Ok(user.filter(|u| u.fingerprints().any(|fp| fp == fingerprint)))
}

/// Same idea as [`resolve_by_fingerprint`], for OIDC email lookup.
pub async fn resolve_by_email(backend: &dyn Backend, cipher: &Cipher, email: &str) -> Result<Option<User>> {
    Ok(User::find_by_email(backend, cipher, email)
        .await?
        .filter(|u| u.email.as_deref() == Some(email)))
}

/// Claims the fingerprint index entry for `username`, self-healing a stale
/// ("phantom") claim left by a crash between the index write and the
/// user-object write in a previous `add_pubkey` (see this module's
/// consistency notes). Returns `Ok(true)` once the claim is `username`'s —
/// caller may now add the key to that user's own `pubkeys` — or `Ok(false)`
/// if the fingerprint is genuinely already in use by a *different* real key
/// on another user (reject the add).
pub async fn claim_pubkey(backend: &dyn Backend, cipher: &Cipher, fingerprint: &str, username: &str) -> Result<bool> {
    PUBKEY_INDEX
        .claim_self_healing(
            backend,
            cipher,
            fingerprint,
            &UsernamePointer {
                username: username.to_string(),
            },
            |existing| async move {
                let (user, _etag) = get_user(backend, cipher, &existing.username).await?;
                Ok(user.is_some_and(|u| u.fingerprints().any(|fp| fp == fingerprint)))
            },
        )
        .await
}

/// Best-effort delete of a fingerprint's index entry — call only *after*
/// the fingerprint has already been removed from the owning user's
/// `pubkeys` (the authoritative, fail-closed step). A failure here just
/// leaves a stale hint that the next [`resolve_by_fingerprint`]/
/// [`claim_pubkey`] self-heals.
pub async fn release_pubkey(backend: &dyn Backend, fingerprint: &str) {
    let _ = PUBKEY_INDEX.release(backend, fingerprint).await;
}

/// Normalized SHA256 fingerprint of an openssh line (`ssh-ed25519 AAAA...
/// comment`), ignoring the comment. Used to compare the key an SSH client
/// offers against the ones stored on a [`User`] regardless of
/// comment/whitespace differences.
pub fn key_fingerprint(openssh_line: &str) -> Result<String> {
    let key =
        PublicKey::from_openssh(openssh_line.trim()).with_context(|| format!("invalid pubkey: {openssh_line}"))?;
    Ok(key.fingerprint(HashAlg::Sha256).to_string())
}

/// Fingerprint of an already-parsed key (the one an SSH client offers in
/// `auth_publickey`), same format as [`key_fingerprint`].
pub fn fingerprint_of(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}
