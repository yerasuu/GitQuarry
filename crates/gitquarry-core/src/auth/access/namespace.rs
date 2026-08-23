//! The shared user/group namespace reservation, plus resolving a repo's
//! owner reference to a display name.

use super::groups::resolve_group_uid;
use super::users::resolve_user_uid;
use crate::crypto::Cipher;
use crate::storage::{Backend, INDEX_PREFIX};
use anyhow::Result;
use s3db::{SecondaryIndex, Segment};
use serde::{Deserialize, Serialize};

/// Shared CAS domain across [`crate::auth::entities::User`] and
/// [`crate::auth::entities::Group`] names — a repo namespace is either a
/// username or a group name, never both, so this can't be either entity's
/// own `#[derive(Entity)]` index (that model has no way to express "claimed
/// jointly with a sibling entity's own key field").
///
/// Single-segment names only: [`Segment::Raw`] rejects a `/`, which is what
/// keeps a claim from relocating its pointer object into a sibling
/// namespace. Subgroup *paths* go to [`SUBGROUP_PATH_INDEX`] instead — see
/// its doc comment for why that's a second index rather than this one
/// switched to [`Segment::Base64`].
const NAMESPACE_INDEX: SecondaryIndex = SecondaryIndex::new(INDEX_PREFIX, "namespace", Segment::Raw);

/// The same CAS domain as [`NAMESPACE_INDEX`], for the nested group paths it
/// can't hold: a subgroup's name is its full path (`acme/team`), and
/// [`Segment::Raw`] rejects the `/`.
///
/// Deliberately a second index rather than re-encoding [`NAMESPACE_INDEX`]
/// as [`Segment::Base64`]: that would move every already-stored reservation
/// to a key nothing looks up any more, and until a backfill ran,
/// [`reserve_namespace`] would happily hand an existing username out to a
/// new group. Splitting by shape has no such window — a path containing a
/// `/` can never collide with a single-segment name, so the two indexes
/// partition the domain instead of overlapping it.
const SUBGROUP_PATH_INDEX: SecondaryIndex = SecondaryIndex::new(INDEX_PREFIX, "group-path", Segment::Base64);

/// Which of the two indexes owns `name`'s reservation — by shape, so the
/// answer never depends on lookup order or on what already exists.
fn namespace_index(name: &str) -> &'static SecondaryIndex {
    if name.contains('/') {
        &SUBGROUP_PATH_INDEX
    } else {
        &NAMESPACE_INDEX
    }
}

/// The key a namespace reservation for `name` lives at — what
/// `auth::domain::{user,group}::create` puts into its create-only
/// transaction, so the reservation and the entity object either both land or
/// neither does.
pub fn namespace_reservation_key(name: &str) -> Result<String> {
    namespace_index(name).key(name)
}

/// One namespace-reservation entry: which kind of entity claimed a name —
/// what actually enforces "a repo namespace is either a username or a
/// group, never both" now that users and groups are separate CAS domains
/// (the single blob's own CAS used to give that uniqueness for free; see
/// [`reserve_namespace`]). `pub(crate)`, same reasoning as
/// [`super::pubkey::UsernamePointer`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NamespaceReservation {
    #[serde(default)]
    pub(crate) kind: String,
}

/// Resolves a repo's `RepoConfig::owner` to its current display name —
/// `None` if `owner` is `None`, or if the uid it names no longer resolves
/// (the owning user/group was deleted): the caller just omits the "owner:"
/// line rather than rendering a dead `/_profile/...` link, same
/// fail-closed-on-display choice
/// [`super::users::resolve_user_uid`]/[`super::groups::resolve_group_uid`]
/// already make on their own. Purely for display — nothing security-relevant
/// goes through this; `mirror::hook::can_push_protected` compares uids
/// against a freshly-fetched `User`/`Group` directly instead.
pub async fn resolve_owner_display(
    backend: &dyn Backend,
    cipher: &Cipher,
    owner: Option<&crate::storage::entities::OwnerRef>,
) -> Result<Option<String>> {
    use crate::storage::entities::OwnerRef;
    Ok(match owner {
        Some(OwnerRef::User(uid)) => resolve_user_uid(backend, cipher, uid).await?.map(|u| u.username),
        Some(OwnerRef::Group(uid)) => resolve_group_uid(backend, cipher, uid).await?.map(|g| g.name),
        None => None,
    })
}

/// CAS-reserves `name` in the shared user/group namespace, closing the race
/// that splitting users/groups into separate key prefixes reintroduced:
/// `add_user("acme")` and `add_group("acme")` now CAS-create distinct keys
/// (`.conf/users/acme.json` vs `.conf/groups/acme.json`), so nothing stops
/// both succeeding concurrently without this. `kind` is `"user"`/`"group"`,
/// only for diagnostics — the reservation itself is what matters. Returns
/// `false` if the name is already reserved.
pub async fn reserve_namespace(backend: &dyn Backend, cipher: &Cipher, name: &str, kind: &str) -> Result<bool> {
    namespace_index(name)
        .claim_once(backend, cipher, name, &NamespaceReservation { kind: kind.to_string() })
        .await
}

/// Whether `name` is already reserved as a user or group namespace — what
/// keeps a repo from taking a path a group already occupies. Both live in
/// the same URL space (`/<namespace>/<repo>` vs `/<group>/<subgroup>`), so
/// `acme/team` being both a subgroup and a repo would leave the router with
/// no way to tell which one `/acme/team` means; `AppContext::create_repo`
/// and `domain::repo::rename` check this before claiming a repo name, the
/// mirror of `domain::group::create` checking the repo-name index before
/// claiming a group path.
pub async fn namespace_reserved(backend: &dyn Backend, cipher: &Cipher, name: &str) -> Result<bool> {
    Ok(namespace_index(name)
        .resolve::<NamespaceReservation>(backend, cipher, name)
        .await?
        .0
        .is_some())
}

/// Best-effort delete of a namespace reservation — call only after the
/// corresponding user/group has actually been deleted, or (on a failed
/// `add_user`/`add_group` after the reservation succeeded) to release a
/// reservation nothing ended up using.
pub async fn release_namespace(backend: &dyn Backend, name: &str) {
    let _ = namespace_index(name).release(backend, name).await;
}
