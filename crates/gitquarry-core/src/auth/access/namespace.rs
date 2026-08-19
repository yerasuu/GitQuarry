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
pub(crate) const NAMESPACE_INDEX: SecondaryIndex = SecondaryIndex::new(INDEX_PREFIX, "namespace", Segment::Raw);

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
    NAMESPACE_INDEX
        .claim_once(backend, cipher, name, &NamespaceReservation { kind: kind.to_string() })
        .await
}

/// Best-effort delete of a namespace reservation — call only after the
/// corresponding user/group has actually been deleted, or (on a failed
/// `add_user`/`add_group` after the reservation succeeded) to release a
/// reservation nothing ended up using.
pub async fn release_namespace(backend: &dyn Backend, name: &str) {
    NAMESPACE_INDEX.release(backend, name).await;
}
