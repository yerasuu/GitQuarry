//! Data migration 1: folds a bucket's pre-[`Group::members`] membership —
//! `owners` and `teams`, still readable as
//! [`Group::legacy_owners`]/[`Group::legacy_teams`] — into the direct member
//! map, and fills the `user-group` index those groups never wrote.
//!
//! Nothing is broken before it runs: [`Group::role_of`] still honors both
//! legacy fields, so access keeps working exactly as it did. What doesn't
//! work is everything that reads `members` as the whole truth — the member
//! table on a group's page, and "the groups I belong to" (`/_groups`, the
//! repos dashboard's group tab, a group offered as a subgroup's parent),
//! which reads the index rather than scanning every group in the bucket.
//!
//! The one non-obvious step is a team scoped to a *subset* of the group's
//! repos (`Team::repos: Some(..)`): folding it into a group-wide role would
//! silently hand its members that tier on every other repo in the group too.
//! Those become explicit per-repo grants on each member's own `User::repos`
//! instead — the same thing a repo's settings tab writes, and the only
//! lossless home for "this tier, on these repos, for this person".
//!
//! Idempotent, as every migration here must be (see [`super`]): a group with
//! no legacy fields left is skipped, an index entry that already exists is a
//! no-op write, and a per-repo grant is only ever *raised*, so re-running
//! after an interrupted pass can't undo anything.

use super::{Mode, Outcome};
use crate::auth::access;
use crate::auth::entities::{Group, Permission};
use crate::crypto::Cipher;
use crate::storage::uid::Uid;
use crate::storage::{Backend, Cas};
use anyhow::Result;
use std::sync::Arc;

pub async fn run(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>, mode: Mode) -> Result<Outcome> {
    let groups = access::list_all_groups(backend, cipher).await?;
    let mut legacy_groups = 0usize;
    let mut grants = 0usize;
    let mut missing_index = 0usize;
    let mut incomplete = 0usize;

    for group in &groups {
        let members = group.effective_members();
        let has_legacy = !group.legacy_owners.is_empty() || !group.legacy_teams.is_empty();
        if has_legacy {
            legacy_groups += 1;
        }
        for member_uid in members.keys() {
            if !indexed(backend.as_ref(), cipher, member_uid, &group.uid).await? {
                missing_index += 1;
                if mode == Mode::Apply {
                    access::add_user_group_index(backend.as_ref(), cipher, member_uid, &group.uid).await;
                }
            }
        }
        for (user_uid, repo_uid, perm) in scoped_team_grants(group) {
            grants += 1;
            if mode == Mode::Apply && !grant_repo_permission(backend, cipher, &user_uid, &repo_uid, perm).await? {
                incomplete += 1;
            }
        }
        if !has_legacy {
            continue;
        }
        if mode == Mode::Apply && !fold_members(backend, cipher, group).await? {
            incomplete += 1;
        }
    }

    let summary = format!(
        "{} group(s) checked -- {legacy_groups} with pre-members owners/teams, {grants} scoped-team grant(s), \
         {missing_index} missing user-group index pointer(s)",
        groups.len()
    );
    Ok(Outcome {
        summary,
        changed: legacy_groups + grants + missing_index,
        incomplete,
    })
}

/// Rewrites one group with its legacy membership folded in. `false` if a
/// concurrent edit won the CAS — the group's legacy fields are then still
/// intact and still granting, so the caller reports it as incomplete and the
/// next pass picks it up.
///
/// Re-reads the group instead of writing the copy the caller already has:
/// the listing that produced it is a bulk read with no etag, and folding is
/// a pure function of whatever the current record says.
async fn fold_members(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>, group: &Group) -> Result<bool> {
    let (fresh, etag) = access::get_group(backend.as_ref(), cipher, &group.name).await?;
    let Some(mut fresh) = fresh else {
        // Deleted between the listing and now: nothing left to migrate, and
        // not a failure either.
        return Ok(true);
    };
    if fresh.legacy_owners.is_empty() && fresh.legacy_teams.is_empty() {
        return Ok(true);
    }
    fresh.members = fresh.effective_members();
    fresh.legacy_owners.clear();
    fresh.legacy_teams.clear();
    Ok(matches!(
        access::put_group(backend.as_ref(), cipher, &fresh, etag.as_deref()).await?,
        Cas::Ok
    ))
}

/// The `(user, repo, tier)` triples a group's subset-scoped legacy teams
/// grant. A team with `repos: None` covered the whole group and is already
/// folded into the member map by [`Group::effective_members`], so it produces
/// nothing here.
pub fn scoped_team_grants(group: &Group) -> Vec<(Uid, Uid, Permission)> {
    let mut out = Vec::new();
    for team in &group.legacy_teams {
        let Some(repos) = &team.repos else { continue };
        for member in &team.members {
            for repo_uid in repos {
                out.push((member.clone(), repo_uid.clone(), team.permission));
            }
        }
    }
    out
}

/// Raises `user_uid`'s explicit grant on `repo_uid` to at least `perm`, never
/// lowering one they already hold: this migration only ever preserves
/// access, so a user separately granted `Maintainer` on a repo their old team
/// saw as `Reporter` keeps the higher tier. That's also what makes it safe to
/// re-run.
///
/// `false` if a concurrent edit to that user won the CAS.
async fn grant_repo_permission(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    user_uid: &Uid,
    repo_uid: &Uid,
    perm: Permission,
) -> Result<bool> {
    let Some(user) = access::resolve_user_uid(backend.as_ref(), cipher, user_uid).await? else {
        // A team member whose account is gone: nothing to grant, and the
        // group rewrite drops the orphaned uid anyway.
        return Ok(true);
    };
    let (stored, etag) = access::get_user(backend.as_ref(), cipher, &user.username).await?;
    let Some(mut stored) = stored else { return Ok(true) };
    if stored.repos.get(repo_uid).copied() >= Some(perm) {
        return Ok(true);
    }
    stored.repos.insert(repo_uid.clone(), perm);
    if !matches!(
        access::put_user(backend.as_ref(), cipher, &stored, etag.as_deref()).await?,
        Cas::Ok
    ) {
        return Ok(false);
    }
    access::add_repo_permission_index(backend.as_ref(), cipher, repo_uid, user_uid).await;
    Ok(true)
}

/// Whether `user_uid` already has a pointer to `group_uid`. One listing per
/// member is more work than a point read would be, but there's no
/// "does this pointer exist" accessor and this runs once per bucket over a
/// member count a group page already renders in full.
async fn indexed(backend: &dyn Backend, cipher: &Cipher, user_uid: &Uid, group_uid: &Uid) -> Result<bool> {
    Ok(access::list_groups_for_user(backend, cipher, user_uid)
        .await?
        .iter()
        .any(|g| &g.uid == group_uid))
}
