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
use russh::keys::ssh_key::SshSig;
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
/// Who a commit's signature actually belongs to.
///
/// Two genuinely different claims, which is why this isn't just an
/// `Option<String>`: a [`Self::User`] signature says *this person wrote
/// this*, checked against a key only they hold the private half of.
/// [`Self::Instance`] says *this server built this* — true of the merge
/// commits it makes on someone's behalf, and all it can honestly say,
/// since the server signs with a key of its own (see
/// [`crate::git::signing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitSigner {
    User(String),
    Instance,
}

pub async fn verified_signer(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    git_dir: &Path,
    sha: &str,
    signature_status: char,
    author_email: &str,
) -> Result<Option<CommitSigner>> {
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
        return Ok(cached.signer());
    }
    let signer = compute_signer(backend, cipher, git_dir, sha, author_email).await?;
    store(backend, cipher, sha, &signer, epoch, etag.as_deref()).await;
    Ok(signer)
}

/// [`verified_signer`] for a request that must not write: the cached
/// verdict if there is a current one, and never a `gpg` spawn or a bucket
/// write either way.
///
/// The nested option is the whole point — `None` is a cache *miss*,
/// `Some(None)` is a stored "checked, and it verifies against nobody".
/// Collapsing the two would re-warm every unverifiable commit on every
/// single page view, forever.
///
/// The read-only variant exists because rendering a commit list used to be
/// a write path — one `CommitSigCache` read *and* one CAS put per row, so
/// an anonymous view of a 50-commit log page issued 100 storage operations
/// before it could answer. [`warm_signer`] is how a miss eventually becomes
/// a hit; until it does the row renders with the neutral "signed" badge
/// rather than the exact verdict.
pub async fn cached_signer(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    sha: &str,
    signature_status: char,
) -> Result<Option<Option<CommitSigner>>> {
    if signature_status == 'N' {
        return Ok(Some(None));
    }
    let epoch = signing_epoch(backend.as_ref(), cipher).await?;
    let (cached, _etag) = CommitSigCache::get(backend.as_ref(), cipher, sha).await?;
    Ok(cached.filter(|c| c.epoch == epoch).map(|c| c.signer()))
}

/// Shas a [`warm_signer`] task is already working on, so a page that shows
/// the same unverified commit in fifty rows — or fifty concurrent viewers
/// of the same page — computes it once. Process-wide because the cache it
/// fills is bucket-wide (keyed by sha and signing epoch, not by repo).
static WARMING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// Bounds how many signature verifications run at once. Each one can shell
/// out to `gpg` with a scratch homedir, so this is deliberately far below
/// the number of rows a single page can ask about: warming is background
/// work that must never compete with the requests it exists to speed up.
static WARM_SLOTS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));

/// Computes and stores `sha`'s verdict outside the request that noticed it
/// was missing, so the next view of that commit is a cache hit. Returns
/// immediately; a failure is logged and dropped, since the only cost is
/// another miss next time.
pub fn warm_signer(
    backend: Arc<dyn Backend>,
    cipher: Arc<Cipher>,
    git_dir: std::path::PathBuf,
    sha: String,
    author_email: String,
) {
    {
        let mut warming = WARMING.lock().expect("signature warm set poisoned");
        if !warming.insert(sha.clone()) {
            return;
        }
    }
    let slots = Arc::clone(&WARM_SLOTS);
    tokio::spawn(async move {
        let result = async {
            let _permit = slots.acquire().await?;
            let epoch = signing_epoch(backend.as_ref(), &cipher).await?;
            let (cached, etag) = CommitSigCache::get(backend.as_ref(), &cipher, &sha).await?;
            // Re-checked under the permit: by the time a slot frees up
            // another instance may have stored the same verdict.
            if cached.is_some_and(|c| c.epoch == epoch) {
                return anyhow::Ok(());
            }
            let signer = compute_signer(&backend, &cipher, &git_dir, &sha, &author_email).await?;
            store(&backend, &cipher, &sha, &signer, epoch, etag.as_deref()).await;
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = result {
            tracing::debug!(error = %e, sha = %sha, "couldn't warm a commit signature");
        }
        WARMING.lock().expect("signature warm set poisoned").remove(&sha);
    });
}

/// Writes one verdict into [`CommitSigCache`]. Best-effort: a lost CAS race
/// just means another concurrent request computed and stored the identical
/// answer first (same sha, same epoch, same deterministic result) — nothing
/// to reconcile.
async fn store(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    sha: &str,
    signer: &Option<CommitSigner>,
    epoch: u64,
    etag: Option<&str>,
) {
    let entry = CommitSigCache {
        sha: sha.to_string(),
        verified_username: match signer {
            Some(CommitSigner::User(username)) => Some(username.clone()),
            _ => None,
        },
        instance: matches!(signer, Some(CommitSigner::Instance)),
        epoch,
    };
    let _ = entry.put(backend.as_ref(), cipher, etag).await;
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
) -> Result<Option<CommitSigner>> {
    let Some((payload, signature)) = git::browse::commit_raw_signature(git_dir, sha).await? else {
        return Ok(None);
    };
    // Checked before any account lookup, and by comparing key material
    // rather than trusting the committer line: a commit can *claim* the
    // instance's committer address, but only the instance's own key
    // produces a signature that matches it.
    if signed_by_this_instance(backend, cipher, &payload, signature.as_bytes()).await? {
        return Ok(Some(CommitSigner::Instance));
    }
    let Some(user) = access::resolve_by_any_email(backend.as_ref(), cipher, author_email).await? else {
        return Ok(None);
    };
    let keys: Vec<String> = user.gpg_keys.iter().map(|k| k.armored.clone()).collect();
    if gitquarry_gpg::verify_detached_signature(&payload, &signature, &keys).await? {
        Ok(Some(CommitSigner::User(user.username)))
    } else {
        Ok(None)
    }
}

/// `true` when `signature` is an SSH signature made by this bucket's own
/// signing key — the key comparison *and* the cryptographic check, both in
/// process.
///
/// The check used to be git's: `%G?` had already validated the signature
/// against the mirror's allowed-signers file, leaving only "whose key is
/// it" to answer here. Nothing asks git for a grade any more (see
/// [`git::browse::CommitInfo::signature_status`] for why), so this verifies
/// the signature itself rather than trusting a commit that merely *claims*
/// the instance's key — an Ed25519 verify, microseconds, no subprocess.
async fn signed_by_this_instance(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    payload: &[u8],
    signature: &[u8],
) -> Result<bool> {
    let Ok(text) = std::str::from_utf8(signature) else {
        return Ok(false);
    };
    let Ok(sig) = SshSig::from_pem(text.as_bytes()) else {
        // An OpenPGP signature, which is every user-signed commit — not an
        // error, just not this branch's case.
        return Ok(false);
    };
    let key = crate::git::signing::load_or_create(backend.as_ref(), cipher).await?;
    let signer = russh::keys::ssh_key::PublicKey::from(sig.public_key().clone());
    let Ok(signer) = signer.to_openssh() else {
        return Ok(false);
    };
    if signer.trim() != key.openssh_public.trim() {
        return Ok(false);
    }
    // `git`'s own namespace for commit signatures — the same string
    // `git commit -S` passes to `ssh-keygen -Y sign -n git`, and part of
    // what's signed, so a signature made for any other namespace must not
    // pass here.
    Ok(signer_key(&key).is_some_and(|k| k.verify("git", payload, &sig).is_ok()))
}

/// This instance's public half as an `ssh-key` type, for
/// [`signed_by_this_instance`]'s verify — stored as OpenSSH text
/// (`InstanceKey::openssh_public`), which is what has to be parsed back.
fn signer_key(key: &crate::git::signing::InstanceKey) -> Option<russh::keys::ssh_key::PublicKey> {
    russh::keys::ssh_key::PublicKey::from_openssh(&key.openssh_public).ok()
}
