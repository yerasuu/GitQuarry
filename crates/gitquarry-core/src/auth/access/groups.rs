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

fn user_group_index(user_uid: &Uid) -> s3db::Collection {
    s3db::Collection::new_owned(format!("{}/user-group/{user_uid}", crate::storage::INDEX_PREFIX))
}

/// Marks `user_uid` as a member of `group_uid` — content unused, existence
/// is the fact, same shape as the repo-owner index. Without it, "the groups
/// I belong to" (the `/_groups` list, the group tab of the repos page, the
/// namespaces `/_new` offers) can only be answered by fetching *every* group
/// in the bucket and testing each one's member map, which is exactly the
/// whole-bucket scan these indexes exist to avoid.
///
/// Best-effort, and written by the `&mut Group` mutators
/// (`domain::group::set_member`) rather than after the group's own CAS
/// lands: a lost CAS leaves an entry pointing at a membership that never
/// happened, which [`list_groups_for_user`] filters out by re-reading the
/// group's own member map — hint, never fact, same doctrine as every other
/// index here.
pub async fn add_user_group_index(backend: &dyn Backend, cipher: &Cipher, user_uid: &Uid, group_uid: &Uid) {
    let _ = user_group_index(user_uid)
        .put(backend, cipher, group_uid.as_str(), &(), None)
        .await;
}

pub async fn remove_user_group_index(backend: &dyn Backend, cipher: &Cipher, user_uid: &Uid, group_uid: &Uid) {
    let _ = user_group_index(user_uid)
        .delete(backend, cipher, group_uid.as_str())
        .await;
}

/// The groups `user_uid` is actually a member of, resolved through
/// [`add_user_group_index`] rather than a bucket-wide group scan. A stale
/// entry (a lost CAS on the group write, a membership since removed on a
/// path that missed its index cleanup) resolves to a group whose own
/// `role_of` no longer answers for this user and is dropped here rather than
/// trusted.
///
/// Only *direct* membership: a member of `acme` inherits their tier in
/// `acme/team` (`permissions::effective_permission` walks the name chain for
/// that), but `acme/team` is not a group they belong to — it shows up under
/// its parent, the way GitLab lists a subgroup inside the group rather than
/// beside it.
pub async fn list_groups_for_user(backend: &dyn Backend, cipher: &Cipher, user_uid: &Uid) -> Result<Vec<Group>> {
    let mut groups = Vec::new();
    for name in user_group_index(user_uid).list_names(backend).await? {
        let Some(group) = resolve_group_uid(backend, cipher, &Uid::from(name)).await? else {
            continue;
        };
        if group.role_of(user_uid).is_some() {
            groups.push(group);
        }
    }
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(groups)
}

fn child_group_index(parent_uid: &Uid) -> s3db::Collection {
    s3db::Collection::new_owned(format!("{}/group-child/{parent_uid}", crate::storage::INDEX_PREFIX))
}

/// Marks `child_uid` as nested directly under `parent_uid` — content
/// unused, existence is the fact, same shape as
/// `storage::settings`'s repo-owner index. The only way to enumerate a
/// group's subgroups without listing every group in the bucket, which at any
/// real group count is exactly the scan these indexes exist to avoid.
/// Best-effort, same tolerance as every other index here: a missed write
/// hides a subgroup from its parent's page until something touches it again,
/// never a correctness problem — permission resolution reads the chain off
/// the group's own `name` ([`ancestor_group_names`]), never this index.
pub async fn add_child_group_index(backend: &dyn Backend, cipher: &Cipher, parent_uid: &Uid, child_uid: &Uid) {
    let _ = child_group_index(parent_uid)
        .put(backend, cipher, child_uid.as_str(), &(), None)
        .await;
}

pub async fn remove_child_group_index(backend: &dyn Backend, cipher: &Cipher, parent_uid: &Uid, child_uid: &Uid) {
    let _ = child_group_index(parent_uid)
        .delete(backend, cipher, child_uid.as_str())
        .await;
}

/// The groups nested directly under `parent_uid` — one level, not the whole
/// subtree (a page renders one level at a time, and a recursive walk would
/// be unbounded fan-out). A stale index entry pointing at a deleted group
/// resolves to nothing and is skipped, same "hint, not a fact" doctrine
/// [`resolve_group_uid`] already follows for its own pointer.
pub async fn list_child_groups(backend: &dyn Backend, cipher: &Cipher, parent_uid: &Uid) -> Result<Vec<Group>> {
    let mut children = Vec::new();
    for name in child_group_index(parent_uid).list_names(backend).await? {
        if let Some(group) = resolve_group_uid(backend, cipher, &Uid::from(name)).await? {
            children.push(group);
        }
    }
    children.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(children)
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
