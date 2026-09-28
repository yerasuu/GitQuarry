//! [`AccessToken`] CRUD and secret generation/hashing — federation layer 4.
//! `AccessToken::TOKEN_HASH_INDEX`/`find_by_token_hash`/`token_hash_claim_op`/
//! `release_token_hash` come from `#[entity(index)]` on the struct itself
//! (`s3db-macros`); this module adds the pieces that derive can't generate:
//! secret generation, the owner-scoped listing index, and secret
//! resolution.
//!
//! Deliberately missing the old single-binary `effective_permission_for_token`:
//! that ceiling computation needed a `User`/`Permission`, which this crate
//! never has (`entities::AccessToken`'s own doc comment). The equivalent
//! decision is now two steps split across two processes: this module's
//! [`resolve_by_token`] plus a *local* `token.write`-derived ceiling check
//! (in whichever web handler authenticates a request), then a
//! `POST /_internal/authorize` call to core for the real, live permission —
//! see `docs/federation-binary-split-concept.md` section 3B.

use crate::entities::AccessToken;
use crate::time::{format_unix_secs, now_secs};
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::{Backend, Cipher, Collection};
use sha2::{Digest, Sha256};

const MAX_TOKEN_LABEL_LEN: usize = 128;
const INDEX_PREFIX: &str = ".conf/index";

/// Validates a token's display label — operator/owner-chosen free text
/// ending up in an HTML page, never an S3 key segment for this entity
/// since the key is a fresh [`Uid`], but unbounded length is still worth
/// rejecting up front.
pub fn validate_token_label(label: &str) -> Result<()> {
    if label.trim().is_empty() || label.len() > MAX_TOKEN_LABEL_LEN {
        bail!("empty or too-long token label (max {MAX_TOKEN_LABEL_LEN})");
    }
    if label.contains(|c: char| c.is_control()) {
        bail!("invalid token label (control characters)");
    }
    Ok(())
}

/// 160 bits of `rand::random`, hex-encoded, prefixed so a token is
/// recognizable at a glance (same idea as GitHub's `ghp_`/`github_pat_`
/// prefixes) — wider than [`Uid::new`]'s 128 bits since this is the actual
/// bearer secret, not just a collision-resistant identifier.
pub fn generate_secret() -> String {
    let raw: [u8; 20] = rand::random();
    format!("token_{}", encode_hex(&raw))
}

/// SHA256 hex of a token secret — a plain fast digest, not Argon2: see
/// [`AccessToken::token_hash`]'s doc comment for why.
pub fn hash_token(secret: &str) -> String {
    encode_hex(&Sha256::digest(secret.as_bytes()))
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn owner_index(owner: &str) -> Collection {
    Collection::new_owned(format!("{INDEX_PREFIX}/token-owner/{owner}"))
}

/// Best-effort: a missed write just hides a token from its owner's
/// "my tokens" listing until the next write touches this index again —
/// never a correctness problem, since [`resolve_by_token`] never consults
/// it.
pub(crate) async fn add_owner_index(backend: &dyn Backend, cipher: &Cipher, owner: &str, token_uid: &Uid) {
    let _ = owner_index(owner)
        .put(backend, cipher, token_uid.as_str(), &(), None)
        .await;
}

pub(crate) async fn remove_owner_index(backend: &dyn Backend, cipher: &Cipher, owner: &str, token_uid: &Uid) {
    let _ = owner_index(owner).delete(backend, cipher, token_uid.as_str()).await;
}

/// Every [`AccessToken`] `owner` currently owns, via the owner index then a
/// concurrent fan-out of per-token GETs. A token count per user is expected
/// to stay small, so no separate cache on top of this.
pub async fn list_tokens_for_owner(backend: &dyn Backend, cipher: &Cipher, owner: &str) -> Result<Vec<AccessToken>> {
    let uids = owner_index(owner).list_names(backend).await?;
    let mut tokens = Vec::with_capacity(uids.len());
    for uid in uids {
        if let (Some(token), _) = AccessToken::get(backend, cipher, &uid).await? {
            tokens.push(token);
        }
    }
    tokens.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(tokens)
}

pub async fn get_token(backend: &dyn Backend, cipher: &Cipher, uid: &str) -> Result<Option<AccessToken>> {
    Ok(AccessToken::get(backend, cipher, uid).await?.0)
}

/// Resolves a bearer secret to its owning username and the [`AccessToken`]
/// it authenticated as, or `None` if the secret is unknown, revoked, or
/// expired. Re-verifies `token_hash` against the fetched record before
/// trusting it — a stale index entry left by a crashed create/revoke fails
/// closed instead of granting access.
///
/// Deliberately `String`, never a `User`: this crate has no such type
/// (`entities::AccessToken`'s own doc comment) — the caller forwards this
/// username verbatim to core's `POST /_internal/authorize`.
pub async fn resolve_by_token(
    backend: &dyn Backend,
    cipher: &Cipher,
    secret: &str,
) -> Result<Option<(String, AccessToken)>> {
    let hash = hash_token(secret);
    let Some(token) = AccessToken::find_by_token_hash(backend, cipher, &hash).await? else {
        return Ok(None);
    };
    if token.token_hash != hash {
        return Ok(None);
    }
    if let Some(expires_at) = &token.expires_at
        && expires_at.as_str() <= format_unix_secs(now_secs()).as_str()
    {
        return Ok(None);
    }
    let owner = token.owner.clone();
    Ok(Some((owner, token)))
}

/// Best-effort refresh of `token.last_used_at` — called on every successful
/// token authorization; a failure here never blocks the request that
/// triggered it.
pub async fn touch_token_last_used(backend: &dyn Backend, cipher: &Cipher, token: &AccessToken) {
    let Ok((Some(mut fresh), etag)) = AccessToken::get(backend, cipher, token.uid.as_str()).await else {
        return;
    };
    fresh.last_used_at = Some(format_unix_secs(now_secs()));
    let _ = fresh.put(backend, cipher, etag.as_deref()).await;
}
