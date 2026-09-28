//! [`PersonalAccessToken`] CRUD and secret generation/hashing — local,
//! git-smart-HTTP-only credential, separate from federation's own
//! fine-grained `AccessToken`. `PersonalAccessToken::TOKEN_HASH_INDEX`/
//! `find_by_token_hash`/`token_hash_claim_op`/`release_token_hash` come
//! from `#[entity(index)]` on the struct itself (`s3db-macros`); this
//! module adds the pieces that derive can't generate: secret
//! generation, the owner-scoped listing index, and secret resolution.
//! Mirrors `gitquarry-federation`'s `access::tokens` module as closely as
//! the two entities' shapes allow.

use crate::auth::entities::PersonalAccessToken;
use crate::storage::INDEX_PREFIX;
use crate::storage::audit::format_unix_secs;
use crate::storage::uid::Uid;
use anyhow::{Result, bail};
use s3db::{Backend, Cipher, Collection};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PAT_LABEL_LEN: usize = 128;

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Validates a token's display label — operator-chosen free text ending up
/// in an HTML page, never an S3 key segment (the key is a fresh [`Uid`]),
/// but unbounded length is still worth rejecting up front.
pub fn validate_pat_label(label: &str) -> Result<()> {
    if label.trim().is_empty() || label.len() > MAX_PAT_LABEL_LEN {
        bail!("empty or too-long token label (max {MAX_PAT_LABEL_LEN})");
    }
    if label.contains(|c: char| c.is_control()) {
        bail!("invalid token label (control characters)");
    }
    Ok(())
}

/// 160 bits of `rand::random`, hex-encoded, prefixed `gqpat_` — distinct
/// from federation's own `token_` prefix so `git::http::authenticate` can
/// route a Basic-Auth username straight to this module (or skip it) without
/// an extra hash lookup, and so a leaked secret is visibly distinguishable
/// from a federation token in logs/support requests.
pub fn generate_secret() -> String {
    let raw: [u8; 20] = rand::random();
    format!("gqpat_{}", encode_hex(&raw))
}

/// SHA256 hex of a token secret — a plain fast digest, not Argon2: see
/// [`PersonalAccessToken::token_hash`]'s doc comment for why.
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

fn owner_index(owner_uid: &Uid) -> Collection {
    Collection::new_owned(format!("{INDEX_PREFIX}/pat-owner/{owner_uid}"))
}

/// Best-effort: a missed write just hides a token from its owner's "my
/// tokens" listing until the next write touches this index again — never a
/// correctness problem, since [`resolve_by_secret`] never consults it.
pub(crate) async fn add_owner_index(backend: &dyn Backend, cipher: &Cipher, owner_uid: &Uid, token_uid: &Uid) {
    let _ = owner_index(owner_uid)
        .put(backend, cipher, token_uid.as_str(), &(), None)
        .await;
}

pub(crate) async fn remove_owner_index(backend: &dyn Backend, cipher: &Cipher, owner_uid: &Uid, token_uid: &Uid) {
    let _ = owner_index(owner_uid).delete(backend, cipher, token_uid.as_str()).await;
}

/// Every [`PersonalAccessToken`] `owner_uid` currently owns, via the owner
/// index then a per-token GET. A token count per user is expected to stay
/// small, so no separate cache on top of this.
pub async fn list_tokens_for_owner(
    backend: &dyn Backend,
    cipher: &Cipher,
    owner_uid: &Uid,
) -> Result<Vec<PersonalAccessToken>> {
    let uids = owner_index(owner_uid).list_names(backend).await?;
    let mut tokens = Vec::with_capacity(uids.len());
    for uid in uids {
        if let (Some(token), _) = PersonalAccessToken::get(backend, cipher, &uid).await? {
            tokens.push(token);
        }
    }
    tokens.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(tokens)
}

pub async fn get_pat(backend: &dyn Backend, cipher: &Cipher, uid: &str) -> Result<Option<PersonalAccessToken>> {
    Ok(PersonalAccessToken::get(backend, cipher, uid).await?.0)
}

/// Resolves a bearer secret to the [`PersonalAccessToken`] it authenticated
/// as, or `None` if the secret is unknown, revoked, or expired.
/// Re-verifies `token_hash` against the fetched record before trusting it —
/// a stale index entry left by a crashed create/revoke fails closed instead
/// of granting access.
pub async fn resolve_by_secret(
    backend: &dyn Backend,
    cipher: &Cipher,
    secret: &str,
) -> Result<Option<PersonalAccessToken>> {
    let hash = hash_token(secret);
    let Some(token) = PersonalAccessToken::find_by_token_hash(backend, cipher, &hash).await? else {
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
    Ok(Some(token))
}

/// Best-effort refresh of `token.last_used_at` — called on every successful
/// PAT authorization; a failure here never blocks the request that
/// triggered it.
pub(crate) async fn touch_pat_last_used(backend: &dyn Backend, cipher: &Cipher, token: &PersonalAccessToken) {
    let Ok((Some(mut fresh), etag)) = PersonalAccessToken::get(backend, cipher, token.uid.as_str()).await else {
        return;
    };
    fresh.last_used_at = Some(format_unix_secs(now_secs()));
    let _ = fresh.put(backend, cipher, etag.as_deref()).await;
}
