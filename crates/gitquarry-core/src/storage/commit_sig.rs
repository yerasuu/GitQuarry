//! Cached result of verifying one commit's GPG signature against the
//! `auth::entities::User::gpg_keys` of whichever account claims the
//! commit's author email — see
//! [`crate::domain::commit_signature::verified_signer`], the only writer.
//!
//! Global (a fixed `#[entity(collection)]`, not a per-repo `Collection`
//! like [`super::lfs_locks`] needs): a commit sha is content-addressed, so
//! the same sha always means the exact same bytes — the same signature —
//! everywhere it shows up, mirror or fork included. Scoping this per-repo
//! would just mean re-verifying (and re-storing) the identical answer once
//! per repo that happens to share history, for no benefit.
//!
//! The sha alone is *not* enough to key the answer, though: the verdict
//! also depends on who claims the author email and which keys that account
//! has registered, and both change over time. [`SIGNING_EPOCH`] is the
//! version stamp for exactly that — every mutation that could flip a
//! verdict bumps it, and an entry stamped with an older epoch is ignored
//! rather than trusted. Without it a `None` cached before the signer
//! registered their key would keep the commit "unverified" forever, and —
//! worse — a `Some(user)` would keep vouching for a key that account has
//! since removed.

use crate::crypto::Cipher;
use crate::storage::Backend;
use anyhow::Result;
use s3db::{Entity, Singleton};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, Entity)]
#[entity(collection = ".conf/commit-sig")]
pub struct CommitSigCache {
    #[serde(default)]
    #[entity(key)]
    pub sha: String,
    /// `None`: unsigned, or signed but no registered account's `gpg_keys`
    /// verifies it. `Some(username)`: that account's own key verified it —
    /// same "author-email-first candidate, then GPG-verify" resolution
    /// `webui::repo::commit::registered_key_signer` used before this cache
    /// existed.
    #[serde(default)]
    pub verified_username: Option<String>,
    /// [`signing_epoch`]'s value when this verdict was computed. An entry
    /// whose stamp no longer matches is a miss, not a hit: some account's
    /// keys or claimed emails changed since, so the verdict has to be
    /// recomputed. `#[serde(default)]` (`0`) for entries written before
    /// this field existed — the epoch counter starts at 1, so every one of
    /// them reads as stale exactly once and then re-stamps itself.
    #[serde(default)]
    pub epoch: u64,
    /// The signature was this instance's own, not any account's — see
    /// `domain::commit_signature::CommitSigner`. A separate flag rather
    /// than a sentinel in `verified_username`, because the instance is not
    /// an account and must never be looked up as one.
    #[serde(default)]
    pub instance: bool,
}

impl CommitSigCache {
    /// The cached verdict in the shape callers actually want.
    pub fn signer(&self) -> Option<crate::domain::commit_signature::CommitSigner> {
        use crate::domain::commit_signature::CommitSigner;
        if self.instance {
            return Some(CommitSigner::Instance);
        }
        self.verified_username.clone().map(CommitSigner::User)
    }
}

/// Bumped by every identity change that can flip a signature verdict:
/// registering or removing a GPG key, and any change to which emails an
/// account claims (primary, secondary, or the account going away entirely).
///
/// One global counter rather than a per-user stamp: resolving *which* user
/// a cached verdict depended on would mean reading that user back on every
/// cache hit, which is the read the cache exists to avoid. A key change on
/// any account therefore invalidates every cached verdict — coarse, but
/// key changes are rare, re-verifying is a `gpg` spawn already budgeted
/// for a cold cache, and it keeps the hit path a single small read that
/// `s3db::RequestCache` collapses to once per request.
const SIGNING_EPOCH: Singleton = Singleton::new(".conf/index/signing-epoch.json");

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct SigningEpoch {
    #[serde(default)]
    epoch: u64,
}

/// Current value of [`SIGNING_EPOCH`]. `0` until the first
/// [`bump_signing_epoch`] creates the object — which matches the
/// `#[serde(default)]` on [`CommitSigCache::epoch`], so a bucket that has
/// never had a key registered still gets cache hits.
pub async fn signing_epoch(backend: &dyn Backend, cipher: &Cipher) -> Result<u64> {
    let (value, _etag) = SIGNING_EPOCH.get::<SigningEpoch>(backend, cipher).await?;
    Ok(value.epoch)
}

/// +1 on [`SIGNING_EPOCH`], retried a few times on CAS conflict. Unlike
/// `bump_password_login_count`'s single best-effort attempt, a lost bump
/// here isn't a cosmetic drift: it would leave every stale verdict looking
/// current, which is the exact bug this counter exists to prevent. Two
/// concurrent bumps landing the same value is fine — all any reader needs
/// is that the number *changed*.
pub async fn bump_signing_epoch(backend: &dyn Backend, cipher: &Cipher) {
    for _ in 0..3 {
        let Ok((mut value, etag)) = SIGNING_EPOCH.get::<SigningEpoch>(backend, cipher).await else {
            return;
        };
        value.epoch = value.epoch.saturating_add(1);
        if let Ok(s3db::Cas::Ok) = SIGNING_EPOCH.put(backend, cipher, &value, etag.as_deref()).await {
            return;
        }
    }
}
