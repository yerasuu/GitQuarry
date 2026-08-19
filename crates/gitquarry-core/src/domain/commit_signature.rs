//! Commit GPG-signature verification against registered
//! [`crate::auth::entities::User::gpg_keys`], cached by sha *and* signing
//! epoch ([`crate::storage::commit_sig::CommitSigCache`] — see its own doc
//! comment for why a global cache is both simpler and more correct than a
//! per-repo one, and why the sha alone can't key the answer). Split out of `webui::repo::commit`, which used to run
//! this same check inline with no caching at all — fine for the one-commit
//! detail view, too expensive to repeat per row on a commit list.
//!
//! Verified lazily, at first render, never at push time: a `pre-receive`
//! hook blocking on a `gpg` spawn per newly-pushed commit would tax every
//! push (import, rebase, the works) for a check almost nothing reads before
//! someone actually opens a commit/commit-list view.

use crate::auth::access;
use crate::crypto::Cipher;
use crate::git;
use crate::storage::Backend;
use crate::storage::commit_sig::{CommitSigCache, signing_epoch};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

/// `sha`'s verified signer, from the cache if already computed. `Ok(None)`
/// covers three cases the caller never needs to tell apart: unsigned,
/// signed but the author's email doesn't resolve to a registered account,
/// or resolved but that account's own `gpg_keys` don't actually verify it.
///
/// `signature_status` is `git::browse::CommitInfo::signature_status`
/// (`%G?`) — `'N'` short-circuits to `Ok(None)` before touching the cache
/// or spawning anything: an unsigned commit can never verify, so there's
/// nothing worth remembering either.
///
/// Takes `backend`/`cipher` separately rather than `&AppContext` (which
/// isn't `Clone`) so `webui::repo::components::commit_avatars` can spawn
/// one of these per commit on a page concurrently, same `JoinSet`-per-item
/// shape `access::list_repo_permissions` already uses.
pub async fn verified_signer(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    git_dir: &Path,
    sha: &str,
    signature_status: char,
    author_email: &str,
) -> Result<Option<String>> {
    if signature_status == 'N' {
        return Ok(None);
    }
    // Read before the cache lookup, not after: a verdict stamped with an
    // older epoch was computed against key/email state that has since
    // changed, so it's a miss no matter what it says (see
    // `storage::commit_sig`'s own doc comment).
    let epoch = signing_epoch(backend.as_ref(), cipher).await?;
    let (cached, etag) = CommitSigCache::get(backend.as_ref(), cipher, sha).await?;
    if let Some(cached) = cached
        && cached.epoch == epoch
    {
        return Ok(cached.verified_username);
    }

    let verified_username = compute_signer(backend, cipher, git_dir, sha, author_email).await?;
    let entry = CommitSigCache {
        sha: sha.to_string(),
        verified_username: verified_username.clone(),
        epoch,
    };
    // Best-effort: a lost CAS race here just means another concurrent
    // request computed and stored the identical answer first (same sha,
    // same epoch, same deterministic result) — nothing to reconcile.
    let _ = entry.put(backend.as_ref(), cipher, etag.as_deref()).await;
    Ok(verified_username)
}

/// Whichever gitquarry account's own registered `gpg_keys` verifies the
/// commit's signature, resolved by `author_email` first (tries the
/// account's primary email, then every claimed
/// [`crate::auth::entities::User::secondary_emails`] — see
/// `access::resolve_by_any_email`). `None` if the email doesn't resolve to
/// any account, or resolves to one whose keys don't verify it — never tried
/// against any *other* account's keys: the author-email match is what
/// narrows the candidate to begin with, not a bucket-wide scan.
async fn compute_signer(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    git_dir: &Path,
    sha: &str,
    author_email: &str,
) -> Result<Option<String>> {
    let Some((payload, signature)) = git::browse::commit_raw_signature(git_dir, sha).await? else {
        return Ok(None);
    };
    let Some(user) = access::resolve_by_any_email(backend.as_ref(), cipher, author_email).await? else {
        return Ok(None);
    };
    let keys: Vec<String> = user.gpg_keys.iter().map(|k| k.armored.clone()).collect();
    if gitquarry_gpg::verify_detached_signature(&payload, &signature, &keys).await? {
        Ok(Some(user.username))
    } else {
        Ok(None)
    }
}
