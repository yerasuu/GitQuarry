//! [`Group`] CRUD and listing — same shape as [`super::users`], for groups.

use crate::auth::entities::Group;
use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::Cas;
use crate::storage::uid::Uid;
use anyhow::Result;
use std::sync::Arc;

/// How many nested groups deep a repo namespace may be. Bounds the fan-out
/// of [`ancestor_group_names`]: every permission check on a repo inside a
/// group fetches one `Group` per level, so an unbounded path would let a
/// single crafted namespace turn one request into arbitrarily many GETs.
/// A path deeper than this simply resolves as if it stopped here — the
/// namespace can't exist anyway, since `domain::group::create` refuses to
/// nest past the same limit.
pub const MAX_GROUP_DEPTH: usize = 10;

/// Every group name in `namespace`'s own chain, outermost first and
/// `namespace` itself last: `"a/b/c"` yields `["a", "a/b", "a/b/c"]`. A
/// group's `name` *is* its full path (see [`Group`]'s doc comment), so the
/// chain is readable straight off the name — no `parent`-pointer walk, which
/// on a permission check would serialize one GET per level instead of
/// fanning them out at once.
///
/// Truncated to the outermost [`MAX_GROUP_DEPTH`] levels, never rejected:
/// this feeds a lookup, and a namespace too deep to exist just resolves to
/// nothing.
pub fn ancestor_group_names(namespace: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut path = String::new();
    for segment in namespace.split('/').take(MAX_GROUP_DEPTH) {
        if segment.is_empty() {
            break;
        }
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(segment);
        names.push(path.clone());
    }
    names
}

/// Every existing group in `namespace`'s chain, outermost first — what
/// `permissions::effective_permission` walks to resolve inherited
/// membership. A name in the chain with no group behind it is skipped
/// rather than failing the lookup: `"alice/proj"`'s namespace is a username,
/// not a group, and a mid-chain gap can't happen through
/// `domain::group::create` (which requires the parent to exist) but must not
/// be fatal if the bucket somehow holds one.
pub async fn ancestor_group_chain(backend: &dyn Backend, cipher: &Cipher, namespace: &str) -> Result<Vec<Group>> {
    let names = ancestor_group_names(namespace);
    let mut chain = Vec::with_capacity(names.len());
    // Sequential rather than a `JoinSet`: the chain is at most
    // `MAX_GROUP_DEPTH` long and is almost always a single name (a
    // top-level group), and both `CachedBackend` and the per-request cache
    // already collapse the repeat GETs a busy page makes for the same
    // group.
    for name in names {
        if let (Some(group), _etag) = get_group(backend, cipher, &name).await? {
            chain.push(group);
        }
    }
    Ok(chain)
}

/// [`super::users::resolve_user_uid`], for [`Group`].
pub async fn resolve_group_uid(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<Option<Group>> {
    Ok(Group::find_by_uid(backend, cipher, uid.as_str())
        .await?
        .filter(|g| &g.uid == uid))
}

/// Same shape as [`super::users::get_user`]/[`super::users::put_user`], for [`Group`].
pub async fn get_group(backend: &dyn Backend, cipher: &Cipher, name: &str) -> Result<(Option<Group>, Option<String>)> {
    Group::get(backend, cipher, name).await
}

pub async fn put_group(
    backend: &dyn Backend,
    cipher: &Cipher,
    group: &Group,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    group.put(backend, cipher, expected_etag).await
}

/// Best-effort delete of a group's own object — same authoritative-step
/// reasoning as [`super::users::delete_user_object`].
pub async fn delete_group_object(backend: &dyn Backend, cipher: &Cipher, name: &str) {
    let _ = Group::delete(backend, cipher, name).await;
}

/// [`super::users::list_usernames`], for [`Group`]s.
pub async fn list_group_names(backend: &dyn Backend) -> Result<Vec<String>> {
    Group::list_keys(backend).await
}

/// [`super::users::list_all_users`], for [`Group`]s.
pub async fn list_all_groups(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>) -> Result<Vec<Group>> {
    let mut groups: Vec<Group> = Group::COLLECTION.list_all(backend, cipher).await?;
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(groups)
}
