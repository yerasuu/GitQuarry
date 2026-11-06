//! Data migration 2: fills the repo permission index for every grant a
//! user already holds in [`User::repos`](crate::auth::entities::User::repos).
//!
//! Before 0.6.0, a grant made from the admin user page
//! (`auth::domain::user::set_repo_perm`) or at account creation wrote only
//! the user's own `repos` map. Access was granted all the same, but the
//! repo's permission grid lists grantees from the index, so those grants were
//! invisible there — and couldn't be revoked from the repo.
//!
//! Idempotent: an entry already in the index is left alone, and nothing is
//! ever removed — a stale entry is already filtered out on read by
//! [`access::list_repo_permissions`].

use super::{Mode, Outcome};
use crate::auth::access;
use crate::crypto::Cipher;
use crate::storage::Backend;
use anyhow::Result;
use std::sync::Arc;

pub async fn run(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>, mode: Mode) -> Result<Outcome> {
    let mut missing = 0usize;
    for user in access::list_all_users(backend, cipher).await? {
        for repo_uid in user.repos.keys() {
            let indexed = access::list_repo_permission_uids(backend.as_ref(), repo_uid).await?;
            if indexed.contains(&user.uid) {
                continue;
            }
            missing += 1;
            if mode == Mode::Apply {
                access::add_repo_permission_index(backend.as_ref(), cipher, repo_uid, &user.uid).await;
            }
        }
    }
    if missing == 0 {
        return Ok(Outcome::clean());
    }
    Ok(Outcome {
        summary: format!("{missing} repo grant(s) added to the repo permission index"),
        changed: missing,
        incomplete: 0,
    })
}
