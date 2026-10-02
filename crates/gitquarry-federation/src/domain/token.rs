//! Every mutation of an [`crate::entities::AccessToken`] goes through here —
//! federation layer 4. Same `domain`/`web` boundary [`crate::domain::peer`]'s
//! doc comment describes.

use crate::access;
use crate::entities::AccessToken;
use crate::time::{format_unix_secs, now_secs};
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::{Backend, Cas, Cipher, txn};
use std::collections::BTreeSet;

/// Creates a token owned by `owner`, scoped to `repos` (already resolved to
/// core's own repo uids — see this crate's doc comment on why a *name* is
/// never resolved here: that needs a live lookup against core, which the
/// caller — a web handler with a `core_client::CoreClient` in hand — does
/// before calling this) plus a `write` ceiling and an optional expiry.
/// Returns the stored record and the one-time plaintext secret — the only
/// moment it's ever available; only `AccessToken::token_hash` survives past
/// this call. `Ok(None)` only on a fresh-uid collision (`Cas::Conflict`,
/// vanishingly rare with 160 bits of randomness). The entity write and its
/// `token_hash` index claim land in one create-only [`txn::run`] batch.
pub async fn create(
    backend: &dyn Backend,
    cipher: &Cipher,
    owner: &str,
    label: &str,
    repos: BTreeSet<Uid>,
    write: bool,
    expires_in_days: Option<u32>,
) -> Result<Option<(AccessToken, String)>> {
    access::validate_token_label(label)?;
    if repos.is_empty() {
        bail!("a token needs at least one repo");
    }

    let secret = access::generate_secret();
    let expires_at = expires_in_days.map(|days| format_unix_secs(now_secs() + u64::from(days) * 86400));
    let token = AccessToken {
        uid: Uid::new(),
        owner: owner.to_string(),
        label: label.to_string(),
        token_hash: access::hash_token(&secret),
        repos,
        write,
        created_at: format_unix_secs(now_secs()),
        expires_at,
        last_used_at: None,
    };

    let ops = vec![
        token.create_op()?,
        token
            .token_hash_claim_op()?
            .expect("token_hash is never empty, always claims"),
    ];
    let cas = txn::run(backend, cipher, ops).await?;
    if cas == Cas::Conflict {
        return Ok(None);
    }
    access::add_owner_index(backend, cipher, owner, &token.uid).await;
    Ok(Some((token, secret)))
}

/// Every token `owner` currently owns — for the "list mine" tab.
pub async fn list_for_owner(backend: &dyn Backend, cipher: &Cipher, owner: &str) -> Result<Vec<AccessToken>> {
    access::list_tokens_for_owner(backend, cipher, owner).await
}

/// Best-effort, a no-op if `uid` doesn't exist or isn't owned by `owner` —
/// never lets one user revoke another's token by guessing a uid. Deletes
/// the token object first (fail-closed: [`access::resolve_by_token`] stops
/// finding it the instant this returns), then releases its `token_hash`
/// index entry and owner-listing entry.
pub async fn revoke(backend: &dyn Backend, cipher: &Cipher, owner: &str, uid: &str) -> Result<()> {
    let Some(token) = access::get_token(backend, cipher, uid).await? else {
        return Ok(());
    };
    if token.owner != owner {
        return Ok(());
    }
    let _ = AccessToken::delete(backend, cipher, uid).await;
    let _ = token.release_token_hash(backend).await;
    access::remove_owner_index(backend, cipher, owner, &token.uid).await;
    Ok(())
}
