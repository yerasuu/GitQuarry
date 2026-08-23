//! `gitquarry migrate-group-members`: one-time operator command that folds a
//! bucket's pre-`Group::members` membership — `owners` and `teams`, kept
//! readable as `Group::legacy_owners`/`legacy_teams` — into the direct
//! member map, and populates the `user-group` index those groups never wrote.
//!
//! Nothing breaks without it: `Group::role_of` still honors both legacy
//! fields, so access keeps working exactly as before. What doesn't work is
//! everything that reads `members` as the whole truth — the member table on
//! a group's page, and "the groups I belong to" (`/_groups`, the repos
//! dashboard's group tab, a group offered as a subgroup's parent), which
//! reads the index rather than scanning every group in the bucket.
//!
//! The one non-obvious step is a team scoped to a *subset* of the group's
//! repos (`Team::repos: Some(..)`): folding it into a group-wide role would
//! silently hand its members that tier on every other repo in the group too.
//! Those become explicit per-repo grants on each member's own `User::repos`
//! instead — the same thing a repo's settings tab writes, and the only
//! lossless home for "this tier, on these repos, for this person".
//!
//! Same dry-run-by-default / `--apply` shape as `fsck`/`repack`/
//! `backfill-repo-index`. Idempotent: a group with no legacy fields left is
//! skipped, and an index entry that already exists is a no-op, so re-running
//! after adding more members is harmless.

use anyhow::{Context, Result, bail};
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Group, Permission, User};
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::{config, storage};
use std::collections::BTreeMap;
use std::sync::Arc;

const USAGE: &str = "\
usage:
  gitquarry migrate-group-members [--bucket NAME] [--apply]

Folds each group's pre-members 'owners'/'teams' into its direct member map
and fills in the .conf/index/user-group/ pointers those groups never wrote.
A team scoped to only some of the group's repos becomes an explicit per-repo
grant on each of its members instead of a group-wide role, so nobody gains
access to a repo that team never covered.

Without --apply it only reports what would change, without writing anything.";

/// One group's worth of planned change — computed before anything is
/// written so `--apply` and the dry-run report can't disagree about what
/// this command does.
struct GroupPlan {
    group: Group,
    etag: Option<String>,
    /// The member map the group ends up with — `Group::effective_members`,
    /// i.e. the direct entries raised by whatever the legacy fields granted
    /// group-wide.
    members: BTreeMap<Uid, Permission>,
    /// `(user_uid, repo_uid, tier)` from the subset-scoped teams, which have
    /// no group-wide equivalent.
    repo_grants: Vec<(Uid, Uid, Permission)>,
}

pub async fn run(rest: &[String]) -> Result<()> {
    let mut bucket_arg: Option<String> = None;
    let mut apply = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => bucket_arg = Some(it.next().cloned().context("--bucket needs a value")?),
            "--apply" => apply = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }

    let config_path = config::Config::resolve_path()?;
    let conf =
        config::Config::load_from(&config_path).with_context(|| format!("couldn't read {}", config_path.display()))?;
    let bucket = match bucket_arg {
        Some(b) => b,
        None => conf
            .only_bucket_name()
            .map(str::to_string)
            .context("--bucket NAME required (config has more than one bucket)")?,
    };
    let bucket_cfg = conf.bucket(&bucket)?.clone();
    let backend = storage::build_backend(&bucket_cfg)?;
    let cipher = Arc::new(
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?,
    );

    let groups = access::list_all_groups(&backend, &cipher).await?;
    let mut plans = Vec::new();
    let mut missing_index = 0usize;
    for group in groups {
        let (_fresh, etag) = access::get_group(backend.as_ref(), &cipher, &group.name).await?;
        let members = group.effective_members();
        let repo_grants = scoped_team_grants(&group);
        // The index is per (user, group), so a group whose members are all
        // already indexed needs nothing even if it does carry legacy fields.
        // One listing per member is more work than a point read would be,
        // but there's no "does this pointer exist" accessor and this runs
        // once, by hand, over a member count a group page already renders.
        for member_uid in members.keys() {
            let indexed = access::list_groups_for_user(backend.as_ref(), &cipher, member_uid)
                .await?
                .iter()
                .any(|g| g.uid == group.uid);
            if !indexed {
                missing_index += 1;
            }
        }
        plans.push(GroupPlan {
            group,
            etag,
            members,
            repo_grants,
        });
    }

    let with_legacy = plans
        .iter()
        .filter(|p| !p.group.legacy_owners.is_empty() || !p.group.legacy_teams.is_empty())
        .count();
    let grants: usize = plans.iter().map(|p| p.repo_grants.len()).sum();
    if with_legacy == 0 && missing_index == 0 {
        println!("gitquarry: {} group(s) checked, nothing to migrate.", plans.len());
        return Ok(());
    }
    println!(
        "gitquarry: {} group(s) checked -- {with_legacy} still carry pre-members owners/teams, {grants} scoped-team \
         grant(s) to write as per-repo permissions, {missing_index} user-group index pointer(s) missing.",
        plans.len()
    );
    if !apply {
        println!("(dry-run -- run again with --apply to write the changes)");
        return Ok(());
    }

    for plan in &plans {
        // Per-repo grants first: they live on the *users*, and a failure
        // there must not leave the group already rewritten as though its
        // scoped teams had been converted.
        for (user_uid, repo_uid, perm) in &plan.repo_grants {
            grant_repo_permission(&backend, &cipher, user_uid, repo_uid, *perm).await?;
        }
        for member_uid in plan.members.keys() {
            access::add_user_group_index(backend.as_ref(), &cipher, member_uid, &plan.group.uid).await;
        }
        if plan.group.legacy_owners.is_empty() && plan.group.legacy_teams.is_empty() {
            continue;
        }
        let mut migrated = plan.group.clone();
        migrated.members = plan.members.clone();
        migrated.legacy_owners.clear();
        migrated.legacy_teams.clear();
        match access::put_group(backend.as_ref(), &cipher, &migrated, plan.etag.as_deref()).await? {
            storage::Cas::Ok => {}
            // Someone edited this group between the plan and the write. Its
            // legacy fields are still intact and still granting, so the
            // safe answer is to say so and let a re-run pick it up.
            storage::Cas::Conflict => {
                println!(
                    "gitquarry: warning: '{}' changed while migrating, left alone -- run again.",
                    migrated.name
                );
            }
        }
    }
    println!("gitquarry: group membership migrated.");
    Ok(())
}

/// The `(user, repo, tier)` triples a group's subset-scoped legacy teams
/// grant. A team with `repos: None` covered the whole group and is already
/// folded into the member map by `Group::effective_members`, so it produces
/// nothing here.
///
/// `pub` so `tests/migrate_group_members.rs` can exercise the one decision
/// in this command that isn't a straight write — the rest of `run` needs a
/// live config file and bucket.
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

/// Raises `user_uid`'s explicit grant on `repo_uid` to at least `perm`,
/// never lowering one they already hold — the migration only ever preserves
/// access, so a user who was separately granted `Maintainer` on a repo their
/// old team saw as `Reporter` keeps the higher tier.
async fn grant_repo_permission(
    backend: &Arc<dyn storage::Backend>,
    cipher: &Arc<gitquarry_core::crypto::Cipher>,
    user_uid: &Uid,
    repo_uid: &Uid,
    perm: Permission,
) -> Result<()> {
    let Some(user) = access::resolve_user_uid(backend.as_ref(), cipher, user_uid).await? else {
        // A team member whose account is gone: nothing to grant, and the
        // group rewrite below drops the orphaned uid anyway.
        return Ok(());
    };
    let (stored, etag) = access::get_user(backend.as_ref(), cipher, &user.username).await?;
    let Some(mut stored) = stored else { return Ok(()) };
    let current = stored.repos.get(repo_uid).copied();
    if current >= Some(perm) {
        return Ok(());
    }
    stored.repos.insert(repo_uid.clone(), perm);
    write_user(backend, cipher, &stored, etag.as_deref()).await?;
    access::add_repo_permission_index(backend.as_ref(), cipher, repo_uid, user_uid).await;
    Ok(())
}

async fn write_user(
    backend: &Arc<dyn storage::Backend>,
    cipher: &Arc<gitquarry_core::crypto::Cipher>,
    user: &User,
    etag: Option<&str>,
) -> Result<()> {
    match access::put_user(backend.as_ref(), cipher, user, etag).await? {
        storage::Cas::Ok => Ok(()),
        // Same reasoning as the group write: the legacy team is still
        // granting this tier, so a re-run is the fix, not a forced write
        // over whatever the concurrent edit was.
        storage::Cas::Conflict => {
            println!(
                "gitquarry: warning: '{}' changed while migrating, their per-repo grants were not written -- run again.",
                user.username
            );
            Ok(())
        }
    }
}
