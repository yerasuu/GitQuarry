//! [`Group`] CRUD and listing — same shape as [`super::users`], for groups.

use crate::auth::entities::Group;
use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::Cas;
use crate::storage::uid::Uid;
use anyhow::Result;
use std::sync::Arc;

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
    Group::delete(backend, cipher, name).await;
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
