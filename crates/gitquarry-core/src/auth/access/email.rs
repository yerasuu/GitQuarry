//! Secondary-email index — self-service additional emails
//! ([`crate::auth::entities::User::secondary_emails`]), separate from the
//! single, immutable `email` OIDC identity field
//! ([`super::pubkey::resolve_by_email`]). Same hand-rolled
//! [`s3db::SecondaryIndex`] shape [`super::pubkey::PUBKEY_INDEX`] uses, for
//! the same reason: a `BTreeSet` of values, each independently claimed, is
//! outside what `#[entity(index)]`'s one-field-one-index derive can express.

use super::users::get_user;
use crate::auth::entities::User;
use crate::crypto::Cipher;
use crate::storage::{Backend, INDEX_PREFIX};
use anyhow::Result;
use s3db::{SecondaryIndex, Segment};

pub(crate) const SECONDARY_EMAIL_INDEX: SecondaryIndex =
    SecondaryIndex::new(INDEX_PREFIX, "secondary-email", Segment::Base64);

/// Resolves a secondary email to its `User`, or `None` if nothing genuinely
/// claims it — same hint-then-reverify shape
/// [`super::pubkey::resolve_by_fingerprint`] uses.
pub async fn resolve_by_secondary_email(backend: &dyn Backend, cipher: &Cipher, email: &str) -> Result<Option<User>> {
    let (pointer, _etag) = SECONDARY_EMAIL_INDEX
        .resolve::<super::pubkey::UsernamePointer>(backend, cipher, email)
        .await?;
    let Some(pointer) = pointer else { return Ok(None) };
    let (user, _etag) = get_user(backend, cipher, &pointer.username).await?;
    Ok(user.filter(|u| u.secondary_emails.contains(email)))
}

/// Tries the primary `email` first (the common case: most accounts commit
/// with the address they logged in with), then every claimed secondary one
/// — what commit-author-email matching (avatar fallback, GPG-signer
/// candidate resolution) actually wants: "does *any* address this account
/// owns match", not just the one OIDC identity field.
pub async fn resolve_by_any_email(backend: &dyn Backend, cipher: &Cipher, email: &str) -> Result<Option<User>> {
    if let Some(user) = super::pubkey::resolve_by_email(backend, cipher, email).await? {
        return Ok(Some(user));
    }
    resolve_by_secondary_email(backend, cipher, email).await
}

/// Claims the secondary-email index entry for `username`, self-healing a
/// stale claim — same contract [`super::pubkey::claim_pubkey`] has.
/// `Ok(false)` if `email` is already claimed by a *different* user (as
/// either their primary `email` or one of their own `secondary_emails`) —
/// checked via [`resolve_by_any_email`] so a primary-email collision is
/// rejected too, not just a secondary one.
pub async fn claim_secondary_email(
    backend: &dyn Backend,
    cipher: &Cipher,
    email: &str,
    username: &str,
) -> Result<bool> {
    if let Some(existing) = resolve_by_any_email(backend, cipher, email).await? {
        return Ok(existing.username == username);
    }
    SECONDARY_EMAIL_INDEX
        .claim_self_healing(
            backend,
            cipher,
            email,
            &super::pubkey::UsernamePointer {
                username: username.to_string(),
            },
            |existing| async move {
                let (user, _etag) = get_user(backend, cipher, &existing.username).await?;
                Ok(user.is_some_and(|u| u.secondary_emails.contains(email)))
            },
        )
        .await
}

/// Best-effort delete of a secondary email's index entry — call only
/// *after* it's already been removed from the owning user's
/// `secondary_emails`, same ordering [`super::pubkey::release_pubkey`]
/// documents.
pub async fn release_secondary_email(backend: &dyn Backend, email: &str) {
    SECONDARY_EMAIL_INDEX.release(backend, email).await;
}
