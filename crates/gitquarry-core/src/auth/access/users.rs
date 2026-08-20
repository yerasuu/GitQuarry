//! [`User`] CRUD and listing.

use crate::auth::entities::User;
use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::Cas;
use crate::storage::uid::Uid;
use anyhow::{Result, bail};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tokio::task::JoinSet;

const MAX_USERNAME_LEN: usize = 128;

/// Validates a username before it's stored. Used for display (web UI
/// labels, attribution) *and*, since the per-entity storage layout below,
/// as a literal S3 key segment (`.conf/users/<username>.json`) — so `/`
/// and `\` are rejected too, not just control characters. Safe to enforce
/// unconditionally: there is no rename action anywhere in this codebase,
/// only add/delete, so `username` is immutable once created.
pub fn validate_username(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > MAX_USERNAME_LEN {
        bail!("empty or too-long username (max {MAX_USERNAME_LEN})");
    }
    if name.contains(|c: char| c.is_control()) {
        bail!("invalid username (control characters)");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("invalid username (can't contain '/' or '\\\\')");
    }
    Ok(())
}

/// Resolves a user uid to their current `User` — a hint, not a fact:
/// re-verified against the pointed-to user's own `.uid` before being
/// trusted, same doctrine every index in this module follows (see
/// [`super::pubkey::resolve_by_fingerprint`]).
pub async fn resolve_user_uid(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<Option<User>> {
    Ok(User::find_by_uid(backend, cipher, uid.as_str())
        .await?
        .filter(|u| &u.uid == uid))
}

/// Fetches one user by username. `None` means "no such user" — unlike
/// [`crate::storage::get_json`]'s own default-on-404 (fine for a container
/// whose empty default is meaningful; wrong here, where a missing object
/// must resolve to "doesn't exist", never a phantom default `User`). The
/// returned etag (`Some` iff the user exists) is what a follow-up
/// [`put_user`] CAS-writes against — `None` both means "doesn't exist" and
/// is exactly what a create-only write expects.
pub async fn get_user(
    backend: &dyn Backend,
    cipher: &Cipher,
    username: &str,
) -> Result<(Option<User>, Option<String>)> {
    User::get(backend, cipher, username).await
}

/// Batched avatar-mime lookup for a set of usernames — what a list view
/// (`IssueMeta::author`/`MergeRequestMeta::author`: denormalized `String`s,
/// no `User` fetched for them anywhere else today) needs to pick an
/// uploaded avatar over the identicon fallback per row, without holding a
/// full `User` per row the way [`super::repo_index::list_repo_permissions`]'s
/// `JoinSet` does for its own uid-keyed case. A username missing from the
/// returned map means either no such user or no avatar uploaded — both
/// cases a caller treats identically (fall back to the identicon), so
/// there's no reason to distinguish them here.
pub async fn avatar_content_types(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    usernames: impl IntoIterator<Item = String>,
) -> HashMap<String, String> {
    let distinct: BTreeSet<String> = usernames.into_iter().collect();
    let mut set = JoinSet::new();
    for username in distinct {
        let backend = Arc::clone(backend);
        let cipher = Arc::clone(cipher);
        set.spawn(async move {
            let (user, _) = get_user(backend.as_ref(), &cipher, &username).await.ok()?;
            let content_type = user.and_then(|u| u.avatar_content_type)?;
            Some((username, content_type))
        });
    }
    let mut out = HashMap::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Some((username, content_type))) = result {
            out.insert(username, content_type);
        }
    }
    out
}

/// Batched email → `User` lookup for a set of raw git commit-author emails
/// (`CommitInfo::author_email`, straight from `%ae`) — what `webui`'s commit
/// tables need to credit a commit to an actual account (and its avatar)
/// rather than the free-text `author_name`/`author_email` git carries,
/// without a per-row [`super::pubkey::resolve_by_email`] call. Unlike
/// [`avatar_content_types`], the full `User` comes back (not just the
/// avatar mime): a matched commit needs the account's real `username` too,
/// to link/key the avatar `<img>` by — a git commit's `author_name` is
/// free text, never safe to treat as a username. An email with no matching
/// account (most commits, for any repo with outside contributors) is simply
/// absent from the returned map — callers fall back to the plain identicon
/// keyed on `author_name` in that case, same as before this existed.
pub async fn avatar_by_commit_email(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    emails: impl IntoIterator<Item = String>,
) -> HashMap<String, User> {
    let distinct: BTreeSet<String> = emails.into_iter().filter(|e| !e.is_empty()).collect();
    let mut set = JoinSet::new();
    for email in distinct {
        let backend = Arc::clone(backend);
        let cipher = Arc::clone(cipher);
        set.spawn(async move {
            let user = crate::auth::access::resolve_by_any_email(backend.as_ref(), &cipher, &email)
                .await
                .ok()??;
            Some((email, user))
        });
    }
    let mut out = HashMap::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Some((email, user))) = result {
            out.insert(email, user);
        }
    }
    out
}

/// CAS-writes one user's full record to their own key.
pub async fn put_user(backend: &dyn Backend, cipher: &Cipher, user: &User, expected_etag: Option<&str>) -> Result<Cas> {
    user.put(backend, cipher, expected_etag).await
}

/// Best-effort delete of a user's own object — called as the first,
/// authoritative step of deleting a user (every lookup path fails closed
/// immediately, regardless of index-cleanup lag on the entries below).
pub async fn delete_user_object(backend: &dyn Backend, cipher: &Cipher, username: &str) {
    let _ = User::delete(backend, cipher, username).await;
}

/// Every username with a `.conf/users/<name>.json` object — cheap, key-only
/// listing (paginated), no bodies fetched. For admin full-list views
/// (owner/namespace `<select>` dropdowns) that only need names, not whole
/// `User` records — mirrors `crate::storage::remote::list_repo_paths`'s own
/// pagination loop.
pub async fn list_usernames(backend: &dyn Backend) -> Result<Vec<String>> {
    User::list_keys(backend).await
}

/// Every `User`, via [`list_usernames`] then a concurrent fan-out of tiny
/// per-user GETs — same shape as `webui::repo::list_visible_repos`'s own
/// listing+fan-out for repos. What lets full-list admin views (`/_users`,
/// a per-repo permission grid, owner dropdowns) enumerate the whole user
/// base without decrypting one shared blob that grows with it just to do
/// that. A user that fails to fetch (network blip, mid-delete race) is
/// silently dropped from the listing rather than failing the whole page —
/// same fail-open-on-display choice `list_visible_repos` makes for a repo
/// whose config fetch errors. Takes an owned `Arc<dyn Backend>` (not
/// `&dyn Backend`), same reasoning as `storage::audit::list_recent`: each
/// spawned fetch needs its own `'static` clone.
pub async fn list_all_users(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>) -> Result<Vec<User>> {
    let mut users: Vec<User> = User::COLLECTION.list_all(backend, cipher).await?;
    users.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(users)
}
