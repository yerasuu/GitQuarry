use super::uid::Uid;
use super::{Backend, Cas};
use crate::crypto::Cipher;
use anyhow::Result;
use s3db::{Collection, SecondaryIndex, Segment, Singleton};
use serde::{Deserialize, Serialize};

use super::entities::{OwnerRef, RepoConfig, ServerSettings};

/// All of gitquarry's config lives under this folder, in two levels:
/// global (`.conf/global.json`, bucket root) and per repo
/// (`<repo>/.conf/config.json`). `.conf/` is a reserved segment: don't use it
/// as a repo name or as a folder inside one.
const SETTINGS: Singleton = Singleton::new(".conf/global.json");

/// Reads the current global config. The etag is `None` if it doesn't exist
/// yet (first time).
pub async fn get_settings(backend: &dyn Backend, cipher: &Cipher) -> Result<(ServerSettings, Option<String>)> {
    SETTINGS.get(backend, cipher).await
}

/// Compare-and-swap: `expected_etag = None` means "must not exist yet".
pub async fn put_settings(
    backend: &dyn Backend,
    settings: &ServerSettings,
    expected_etag: Option<&str>,
    cipher: &Cipher,
) -> Result<Cas> {
    SETTINGS.put(backend, cipher, settings, expected_etag).await
}

/// Reads the repo's config. The etag is `None` if it doesn't exist yet.
pub async fn get_repo_config(
    backend: &dyn Backend,
    repo: &str,
    cipher: &Cipher,
) -> Result<(RepoConfig, Option<String>)> {
    RepoConfig::COLLECTION.get_or_default(backend, cipher, repo).await
}

/// Compare-and-swap: `expected_etag = None` means "must not exist yet".
pub async fn put_repo_config(
    backend: &dyn Backend,
    repo: &str,
    config: &RepoConfig,
    expected_etag: Option<&str>,
    cipher: &Cipher,
) -> Result<Cas> {
    RepoConfig::COLLECTION
        .put(backend, cipher, repo, config, expected_etag)
        .await
}

/// One repo-name index entry: which uid currently claims a full repo name
/// (`namespace/local-name`). The raw name is safe as a literal key segment
/// without base64 encoding — same reasoning as `auth::access`'s `namespace`
/// index, since every repo name passing through here already went through
/// [`crate::validate::sanitize_path`] (no `..`, no control chars, no `\`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct RepoUidPointer {
    #[serde(default)]
    uid: Uid,
}

const REPO_NAME_INDEX: SecondaryIndex = SecondaryIndex::new(super::INDEX_PREFIX, "repo-name", Segment::Raw);

/// `None` if nothing currently claims `full_name` — the read side every
/// existing-repo lookup (browsing, clone, settings, fork's source, star
/// toggle) goes through before touching a repo's own uid-prefixed storage.
pub async fn resolve_repo_name(backend: &dyn Backend, cipher: &Cipher, full_name: &str) -> Result<Option<Uid>> {
    let (pointer, _etag) = REPO_NAME_INDEX
        .resolve::<RepoUidPointer>(backend, cipher, full_name)
        .await?;
    Ok(pointer.map(|p| p.uid))
}

/// CAS-writes the `full_name -> uid` claim. `expected_etag = None` for a
/// fresh claim (fails with `Cas::Conflict` if the name is already taken);
/// `Some(etag)` from a prior [`resolve_repo_name`] to reclaim a name whose
/// existing claim has been confirmed stale (its uid's own repo data no
/// longer exists) — that confirmation needs a live `Remote`, which this
/// module doesn't depend on, so it's the caller's job (see
/// `AppContext::create_repo`), not this function's.
///
/// Test-only: real code always goes through [`claim_repo_name_self_healing`]
/// (`AppContext::create_repo`'s only caller), which never falls back to this
/// plain CAS-create directly.
#[cfg(any(test, feature = "test-util"))]
pub async fn claim_repo_name(
    backend: &dyn Backend,
    cipher: &Cipher,
    full_name: &str,
    uid: &Uid,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    REPO_NAME_INDEX
        .put_raw(
            backend,
            cipher,
            full_name,
            &RepoUidPointer { uid: uid.clone() },
            expected_etag,
        )
        .await
}

/// Best-effort delete of a repo-name claim — call only after the repo
/// itself is gone (`delete_repo`) or once a `move_repo` has finished
/// pointing `full_name` at its new value. A failure here just leaves a
/// stale hint the next [`claim_repo_name`] conflict self-heals through.
pub async fn release_repo_name(backend: &dyn Backend, full_name: &str) {
    REPO_NAME_INDEX.release(backend, full_name).await;
}

/// Self-healing repo-name claim: attempts a fresh CAS-create for `uid`; on
/// conflict, asks `still_live` whether the uid currently holding the claim
/// still has a real repo behind it before either refusing (genuine claim —
/// `Ok(false)`) or reclaiming a stale one (that uid's own data is gone — a
/// deleted repo whose `release_repo_name` never ran, or lost the race with
/// it). `still_live` takes a `&Uid` since checking existence needs a live
/// `Remote`, which this module doesn't depend on (avoids a cycle with
/// `storage::remote`, which already depends on this module for
/// `get_repo_config`) — the caller (`AppContext::create_repo`) supplies it.
pub async fn claim_repo_name_self_healing<F, Fut>(
    backend: &dyn Backend,
    cipher: &Cipher,
    full_name: &str,
    uid: &Uid,
    still_live: F,
) -> Result<bool>
where
    F: FnOnce(&Uid) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    REPO_NAME_INDEX
        .claim_self_healing(
            backend,
            cipher,
            full_name,
            &RepoUidPointer { uid: uid.clone() },
            |existing| async move {
                if existing.uid.is_empty() {
                    return Ok(false); // nothing there at all — not a live claim
                }
                still_live(&existing.uid).await
            },
        )
        .await
}

/// The `Uid` either `OwnerRef` variant wraps, regardless of which.
fn owner_uid(owner: &OwnerRef) -> &Uid {
    match owner {
        OwnerRef::User(uid) | OwnerRef::Group(uid) => uid,
    }
}

fn owner_index(owner: &OwnerRef) -> Collection {
    let (kind, uid) = match owner {
        OwnerRef::User(uid) => ("user", uid),
        OwnerRef::Group(uid) => ("group", uid),
    };
    Collection::new_owned(format!("{}/repo-owner/{kind}/{uid}", super::INDEX_PREFIX))
}

/// Marks `repo_uid` as owned by `owner` — lets `auth::web::profile` list a
/// profile's repos by scanning only `owner_index_prefix(owner)` instead of
/// every repo in the bucket. Best-effort like every other index in this
/// module: a missed write just hides the repo from its owner's profile page
/// until the next write to that repo's config touches this index again,
/// never a correctness problem for anything that already has the repo's own
/// uid (direct URL, clone, settings page). `expected_etag: None` is a
/// create-only CAS; a `Conflict` just means the entry is already there
/// (idempotent retry, or this owner never lost it), so it's fine to ignore.
/// A no-op if `owner`'s own uid is empty (`Uid::default()`, the "no claim"
/// sentinel — see [`Uid::is_empty`]'s doc comment): that's not a real owner
/// to index under, and indexing it anyway would build a key with an empty
/// path segment that [`list_owned_repo_uids`] would still happily read
/// back, silently masking the underlying data problem instead of
/// surfacing it.
pub async fn add_owner_index(backend: &dyn Backend, cipher: &Cipher, owner: &OwnerRef, repo_uid: &Uid) {
    if owner_uid(owner).is_empty() {
        return;
    }
    let _ = owner_index(owner)
        .put(backend, cipher, repo_uid.as_str(), &(), None)
        .await;
}

/// Best-effort like [`add_owner_index`] — call when `repo_uid` stops
/// belonging to `owner` (rename/transfer to a new owner, deletion). Same
/// empty-uid guard as [`add_owner_index`] — nothing valid was ever indexed
/// there to remove.
pub async fn remove_owner_index(backend: &dyn Backend, cipher: &Cipher, owner: &OwnerRef, repo_uid: &Uid) {
    if owner_uid(owner).is_empty() {
        return;
    }
    owner_index(owner).delete(backend, cipher, repo_uid.as_str()).await;
}

/// Every repo uid currently indexed under `owner` — a hint, not a fact: a
/// stale entry (left behind by a failed `remove_owner_index`) can point at a
/// repo that no longer belongs to `owner`, so callers still compare
/// `RepoConfig::owner` themselves once they fetch it, same doctrine
/// `auth::access`'s uid-reverse-indexes already follow.
pub async fn list_owned_repo_uids(backend: &dyn Backend, owner: &OwnerRef) -> Result<Vec<Uid>> {
    if owner_uid(owner).is_empty() {
        return Ok(Vec::new());
    }
    Ok(owner_index(owner)
        .list_names(backend)
        .await?
        .into_iter()
        .map(Uid::from)
        .collect())
}

/// Best-effort `+= delta_bytes` on `uid`'s own `RepoConfig::pack_bytes` —
/// read/modify/CAS-put, same "lost race just undercounts, self-heals"
/// tolerance as `domain::repo::bump_fork_count`. Called once per pack
/// actually uploaded (`mirror::hook::run_pre_receive_hook`).
pub async fn bump_pack_bytes(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, delta_bytes: u64) {
    let Ok((mut config, etag)) = get_repo_config(backend, uid.as_str(), cipher).await else {
        return;
    };
    config.pack_bytes = config.pack_bytes.saturating_add(delta_bytes);
    let _ = put_repo_config(backend, uid.as_str(), &config, etag.as_deref(), cipher).await;
}

/// Same as [`bump_pack_bytes`], for `RepoConfig::lfs_bytes` — called once
/// per genuinely new LFS object upload (the caller only calls this when
/// the oid didn't already exist, see that field's own doc comment).
pub async fn bump_lfs_bytes(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, delta_bytes: u64) {
    let Ok((mut config, etag)) = get_repo_config(backend, uid.as_str(), cipher).await else {
        return;
    };
    config.lfs_bytes = config.lfs_bytes.saturating_add(delta_bytes);
    let _ = put_repo_config(backend, uid.as_str(), &config, etag.as_deref(), cipher).await;
}

/// Sum of `pack_bytes + lfs_bytes` across every repo currently indexed
/// under `owner` — what `auth::access::quota::check` compares against the
/// owner's own limit. One `RepoConfig` fetch per owned repo (a bounded,
/// per-owner count, not a bucket-wide scan) rather than maintaining a
/// second, separate aggregate that would need its own consistency story on
/// top of each repo's own best-effort fields.
pub async fn sum_owner_repo_bytes(backend: &dyn Backend, cipher: &Cipher, owner: &OwnerRef) -> Result<u64> {
    let uids = list_owned_repo_uids(backend, owner).await?;
    let mut total = 0u64;
    for uid in uids {
        if let Ok((config, _etag)) = get_repo_config(backend, uid.as_str(), cipher).await {
            total = total.saturating_add(config.pack_bytes).saturating_add(config.lfs_bytes);
        }
    }
    Ok(total)
}

fn visible_index() -> Collection {
    Collection::new_owned(format!("{}/repo-visible", super::INDEX_PREFIX))
}

/// Keeps `.conf/index/repo-visible/<repo-uid>.json` in sync with whether a
/// repo is currently anything other than `Private` — lets
/// `webui::repo::list_visible_repos` (the site's anonymous index, its highest-
/// traffic page) skip fetching, and thus ever touching, a `Private` repo's
/// config at all, instead of fetching every repo in the bucket just to
/// filter most of them back out. Best-effort, same tolerance as
/// [`add_owner_index`]: a missed write either hides a visible repo from the
/// index (self-heals on the next config write) or, worse but still
/// self-healing, leaves a since-privated repo's uid in the index one write
/// cycle longer — harmless, since `list_visible_repos` re-checks
/// `RepoConfig::visibility` itself before showing anything.
pub async fn set_visible_index(backend: &dyn Backend, cipher: &Cipher, repo_uid: &Uid, visible: bool) {
    if visible {
        let _ = visible_index().put(backend, cipher, repo_uid.as_str(), &(), None).await;
    } else {
        visible_index().delete(backend, cipher, repo_uid.as_str()).await;
    }
}

/// Every repo uid currently indexed as visible — same "hint, not a fact"
/// doctrine as [`list_owned_repo_uids`].
pub async fn list_visible_repo_uids(backend: &dyn Backend) -> Result<Vec<Uid>> {
    Ok(visible_index()
        .list_names(backend)
        .await?
        .into_iter()
        .map(Uid::from)
        .collect())
}

fn deleted_index() -> Collection {
    Collection::new_owned(format!("{}/repo-deleted", super::INDEX_PREFIX))
}

/// Keeps `.conf/index/repo-deleted/<repo-uid>.json` in sync with whether a
/// repo is currently soft-deleted (`RepoConfig::deleted_at` set) — lets
/// `mirror::deleted_repo_sweep` find grace-expired repos by scanning only
/// this small index instead of every repo in the bucket, same reasoning
/// [`set_visible_index`] already has for the site's anonymous listing. Same
/// best-effort tolerance: a missed write either strands an entry here
/// (harmless — the sweep re-checks `RepoConfig::deleted_at` before touching
/// anything) or misses one (the repo just outlives its grace window one
/// sweep cycle longer, self-heals whenever this is called again).
pub async fn set_deleted_index(backend: &dyn Backend, cipher: &Cipher, repo_uid: &Uid, deleted: bool) {
    if deleted {
        let _ = deleted_index().put(backend, cipher, repo_uid.as_str(), &(), None).await;
    } else {
        deleted_index().delete(backend, cipher, repo_uid.as_str()).await;
    }
}

/// Every repo uid currently indexed as soft-deleted — same "hint, not a
/// fact" doctrine as [`list_visible_repo_uids`].
pub async fn list_deleted_repo_uids(backend: &dyn Backend) -> Result<Vec<Uid>> {
    Ok(deleted_index()
        .list_names(backend)
        .await?
        .into_iter()
        .map(Uid::from)
        .collect())
}

fn mirrors_enabled_index() -> Collection {
    Collection::new_owned(format!("{}/mirrors-enabled", super::INDEX_PREFIX))
}

/// Keeps `.conf/index/mirrors-enabled/<repo-uid>.json` in sync with whether
/// this repo currently has at least one enabled external mirror — same
/// "hint, not a fact" doctrine as [`set_visible_index`]. Relocated here from
/// `mirror::external::access` (see `docs/mirror-external-extraction-concept.md`)
/// so `mirror::hook`'s pre-receive path can answer "does this repo have any
/// enabled mirror" — the same question the periodic sync tick already asks
/// via [`list_repos_with_enabled_mirrors`] — without depending on the crate
/// that owns the mirror CRUD/validation surface. The write side lives in
/// that crate, which calls this after every create/update/delete.
/// `Err` only on the enable side, and on purpose: this index is the *only*
/// thing that tells either sync path a repo has a mirror at all
/// (`has_enabled_mirror`, `list_repos_with_enabled_mirrors`), so a lost
/// write there leaves a mirror that looks configured and never syncs, with
/// nothing anywhere saying why. The caller propagates it so the settings
/// tab can. A lost *disable* stays silent, matching the "hint, not a fact"
/// doctrine — a stale entry only costs the sweep one wasted lookup, since
/// it re-reads the repo's real mirrors before syncing anything.
pub async fn set_mirrors_enabled_index(
    backend: &dyn Backend,
    cipher: &Cipher,
    repo_uid: &Uid,
    enabled: bool,
) -> Result<()> {
    if enabled {
        // `Cas::Conflict` means an entry is already there, which is exactly
        // the state being asked for — not a failure.
        mirrors_enabled_index()
            .put(backend, cipher, repo_uid.as_str(), &(), None)
            .await?;
    } else {
        mirrors_enabled_index().delete(backend, cipher, repo_uid.as_str()).await;
    }
    Ok(())
}

/// `true` if [`set_mirrors_enabled_index`] last recorded this repo as having
/// an enabled mirror — a single indexed lookup `mirror::hook` uses instead
/// of listing and decrypting the repo's whole mirrors collection just to
/// check one bool.
pub async fn has_enabled_mirror(backend: &dyn Backend, cipher: &Cipher, repo_uid: &Uid) -> bool {
    mirrors_enabled_index()
        .get::<()>(backend, cipher, repo_uid.as_str())
        .await
        .is_ok_and(|(entry, _)| entry.is_some())
}

/// Every repo uid currently indexed as having at least one enabled external
/// mirror — same "hint, not a fact" doctrine as [`list_visible_repo_uids`]:
/// the periodic sync tick re-fetches and re-filters each repo's actual
/// mirrors before syncing anything, so a stale entry here just costs one
/// wasted lookup, never a wrong sync.
pub async fn list_repos_with_enabled_mirrors(backend: &dyn Backend) -> Result<Vec<Uid>> {
    Ok(mirrors_enabled_index()
        .list_names(backend)
        .await?
        .into_iter()
        .map(Uid::from)
        .collect())
}
