//! [`Passkey`] registration, listing, removal and sign-in verification.
//! Transport-agnostic like the rest of `domain`: the web layer owns the
//! challenge cookie, CSRF and re-authentication; this owns what a stored
//! credential may and may not do.

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{Passkey, User};
use crate::storage::Cas;
use crate::storage::audit::format_unix_secs;
use crate::storage::uid::Uid;
use anyhow::{Result, bail};
use gitquarry_auth_crypto::webauthn::{self, NewCredential, RelyingParty, WebauthnError};

/// Stores a credential [`webauthn::verify_registration`] accepted for `owner`.
///
/// # Errors
///
/// An invalid `label`, `owner` already at [`access::MAX_PASSKEYS_PER_USER`],
/// or a credential id that is already registered (to anyone).
pub async fn register(ctx: &AppContext, owner: &User, credential: NewCredential, label: &str) -> Result<Passkey> {
    access::validate_passkey_label(label)?;
    let backend = ctx.backend();
    if access::list_passkeys_for_owner(backend.as_ref(), &ctx.cipher, &owner.uid)
        .await?
        .len()
        >= access::MAX_PASSKEYS_PER_USER
    {
        bail!("at most {} passkeys per account", access::MAX_PASSKEYS_PER_USER);
    }
    let credential_id = webauthn::encode_id(&credential.credential_id);
    let passkey = Passkey {
        key: access::passkey_key(&credential_id),
        credential_id,
        owner_uid: owner.uid.clone(),
        public_key: webauthn::encode_id(&credential.public_key),
        sign_count: credential.sign_count,
        label: label.trim().to_string(),
        backup_eligible: credential.backup_eligible,
        created_at: format_unix_secs(access::now_secs()),
        last_used_at: None,
    };
    // Create-only: an id already on file stays with whoever registered it.
    if passkey.put(backend.as_ref(), &ctx.cipher, None).await? == Cas::Conflict {
        bail!("this passkey is already registered");
    }
    access::add_passkey_owner_index(backend.as_ref(), &ctx.cipher, &owner.uid, &passkey.key).await;
    Ok(passkey)
}

pub async fn list(ctx: &AppContext, owner_uid: &Uid) -> Result<Vec<Passkey>> {
    access::list_passkeys_for_owner(ctx.backend().as_ref(), &ctx.cipher, owner_uid).await
}

/// Removes `owner_uid`'s passkey stored under `key`. A key that isn't
/// theirs (or doesn't exist) is a no-op, never someone else's removal.
pub async fn remove(ctx: &AppContext, owner_uid: &Uid, key: &str) -> Result<bool> {
    let backend = ctx.backend();
    let (Some(passkey), _) = Passkey::get(backend.as_ref(), &ctx.cipher, key).await? else {
        return Ok(false);
    };
    if &passkey.owner_uid != owner_uid {
        return Ok(false);
    }
    Passkey::delete(backend.as_ref(), &ctx.cipher, key).await?;
    access::remove_passkey_owner_index(backend.as_ref(), &ctx.cipher, owner_uid, key).await;
    Ok(true)
}

/// Every passkey of a user being deleted — best-effort, since a leftover
/// record can't sign in anyway once [`verify`] finds no owner behind it.
pub(crate) async fn remove_all(ctx: &AppContext, owner_uid: &Uid) {
    let Ok(passkeys) = list(ctx, owner_uid).await else {
        return;
    };
    for passkey in passkeys {
        let _ = remove(ctx, owner_uid, &passkey.key).await;
    }
}

/// A `navigator.credentials.get()` response, decoded from its transport
/// encoding by the caller.
pub struct Assertion {
    /// base64url, as the browser's `rawId`.
    pub credential_id: String,
    pub client_data_json: Vec<u8>,
    pub authenticator_data: Vec<u8>,
    pub signature: Vec<u8>,
    /// What the authenticator stored as the user handle at registration —
    /// the owner's uid bytes — returned by discoverable credentials.
    pub user_handle: Option<Vec<u8>>,
}

/// Why a sign-in with a passkey was refused. None of these say anything to
/// the person signing in beyond "that didn't work"; they're for logs.
#[derive(Debug, PartialEq, Eq)]
pub enum PasskeyRefusal {
    UnknownCredential,
    /// A registered passkey, but not the account this step is for.
    NotThisAccount,
    OwnerGone,
    Webauthn(WebauthnError),
    /// Another sign-in with the same passkey moved its counter at the same
    /// moment; refusing is what keeps the counter check meaningful.
    ConcurrentUse,
}

impl std::fmt::Display for PasskeyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasskeyRefusal::UnknownCredential => f.write_str("unknown passkey"),
            PasskeyRefusal::NotThisAccount => f.write_str("passkey belongs to another account"),
            PasskeyRefusal::OwnerGone => f.write_str("passkey's account no longer exists"),
            PasskeyRefusal::Webauthn(e) => e.fmt(f),
            PasskeyRefusal::ConcurrentUse => f.write_str("passkey used concurrently"),
        }
    }
}

/// Verifies `assertion` against the stored credential it names, for
/// `expected_challenge`, and records the new counter.
///
/// `expected_owner`: `Some` when the passkey is the second factor for an
/// account the password already identified — any other account's passkey
/// is refused; `None` for a passkey-only sign-in, where the passkey itself
/// says who is signing in. `require_uv` should be `true` exactly then.
///
/// # Errors
///
/// The outer `Err` is a storage failure; the inner one a refusal.
pub async fn verify(
    ctx: &AppContext,
    rp: &RelyingParty,
    expected_challenge: &str,
    assertion: &Assertion,
    expected_owner: Option<&Uid>,
    require_uv: bool,
) -> Result<Result<(User, Passkey), PasskeyRefusal>> {
    let backend = ctx.backend();
    let Some((mut passkey, etag)) =
        access::find_by_credential_id(backend.as_ref(), &ctx.cipher, &assertion.credential_id).await?
    else {
        return Ok(Err(PasskeyRefusal::UnknownCredential));
    };
    if expected_owner.is_some_and(|owner| owner != &passkey.owner_uid) {
        return Ok(Err(PasskeyRefusal::NotThisAccount));
    }
    if assertion
        .user_handle
        .as_deref()
        .is_some_and(|handle| handle != passkey.owner_uid.as_str().as_bytes())
    {
        return Ok(Err(PasskeyRefusal::NotThisAccount));
    }
    let Ok(public_key) = webauthn::decode_b64url(&passkey.public_key) else {
        bail!("stored passkey {} has an undecodable public key", passkey.key);
    };
    let verified = match webauthn::verify_assertion(
        rp,
        expected_challenge,
        &public_key,
        passkey.sign_count,
        &assertion.client_data_json,
        &assertion.authenticator_data,
        &assertion.signature,
        require_uv,
    ) {
        Ok(verified) => verified,
        Err(e) => return Ok(Err(PasskeyRefusal::Webauthn(e))),
    };
    let Some(owner) = access::resolve_user_uid(backend.as_ref(), &ctx.cipher, &passkey.owner_uid).await? else {
        return Ok(Err(PasskeyRefusal::OwnerGone));
    };
    passkey.sign_count = verified.sign_count;
    passkey.last_used_at = Some(format_unix_secs(access::now_secs()));
    if passkey.put(backend.as_ref(), &ctx.cipher, etag.as_deref()).await? == Cas::Conflict {
        return Ok(Err(PasskeyRefusal::ConcurrentUse));
    }
    Ok(Ok((owner, passkey)))
}
