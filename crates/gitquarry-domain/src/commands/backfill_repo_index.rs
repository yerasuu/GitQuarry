//! `gitquarry backfill-repo-index`: one-time operator command for buckets
//! that already had repos (and repo permission grants) before
//! `.conf/index/repo-owner/`/`.conf/index/repo-visible/`/
//! `.conf/index/repo-permission/` existed (see `domain::repo`'s
//! `create`/`rename`/`fork`/`set_conf`/`set_permission`/`delete`, which keep
//! all three in sync going forward for anything created or edited after
//! this landed). Without this, a repo created before the index existed is
//! invisible on `/_profile/<owner>` and (if not `Private`) on `/`, and a
//! permission granted before the index existed is invisible on the repo's
//! own settings-tab grid — until something happens to touch that repo's
//! config or that user's permission again. A silent disappearance, not an
//! error, so it gets its own explicit fix rather than a lazy fallback that
//! would just bring back the bucket-wide scans these indexes exist to
//! avoid on every page view.
//!
//! Same dry-run-by-default / `--apply` shape as `fsck`/`repack`: lists
//! every repo and every user (`storage::remote::list_repo_paths`/
//! `access::list_all_users` — the same bucket-wide scans the indexes
//! replace on their hot paths, fine here since this runs once, by hand, not
//! on every request), and writes whichever index entries are missing.
//! Idempotent: an already-indexed repo/grant is just skipped, so re-running
//! (e.g. after creating more repos or grants before remembering to
//! backfill) is harmless. Only adds missing entries — doesn't hunt for or
//! remove stale ones, that's already self-healing through `domain::repo`'s
//! own write paths.

use anyhow::{Context, Result, bail};
use gitquarry_core::auth::access;
use gitquarry_core::storage::entities::OwnerRef;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::{config, storage};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const USAGE: &str = "\
usage:
  gitquarry backfill-repo-index [--bucket NAME] [--apply]

Repopulates .conf/index/repo-owner/, .conf/index/repo-visible/ and
.conf/index/repo-permission/ for repos/permissions that already existed
before those indexes did -- without this, such a repo doesn't show up on
/_profile/<owner> nor (if not private) on /, and a permission already
granted doesn't show up in the repo's settings-tab grid, until something
touches that config or that permission again. Idempotent: an already
indexed entry is skipped, running it again does nothing extra.

Without --apply it only counts how many entries are missing, without
writing anything.";

/// `user:<uid>` / `group:<uid>` — just a hashable dedup key so repos
/// sharing an owner only trigger one `list_owned_repo_uids` call each,
/// not one per repo. `OwnerRef` itself isn't `Hash`, and doesn't need to
/// become so just for this one-off tool.
fn owner_dedup_key(owner: &OwnerRef) -> String {
    match owner {
        OwnerRef::User(uid) => format!("user:{uid}"),
        OwnerRef::Group(uid) => format!("group:{uid}"),
    }
}

/// `true` for `Some(OwnerRef::User(Uid::default()))`/`Group(..)` — an empty
/// uid is the "no claim" sentinel (see `Uid::is_empty`'s doc comment), not
/// a real owner: `storage::settings::add_owner_index`/`list_owned_repo_uids`
/// both no-op on one (see their own doc comments), so counting it as
/// "missing" here would report the same repo as needing a backfill on
/// every single run, forever, since nothing this tool does can ever
/// satisfy it. A `RepoConfig::owner` in this shape predates this index
/// entirely and points at a data gap of its own — worth fixing at the
/// source (re-set the repo's owner from its settings tab), not something
/// this tool's job to paper over.
fn owner_is_unset(owner: &OwnerRef) -> bool {
    match owner {
        OwnerRef::User(uid) | OwnerRef::Group(uid) => uid.is_empty(),
    }
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
    let backend = storage::build_backend(&bucket_cfg);
    let cipher = Arc::new(
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?,
    );

    let repo_uids = storage::remote::list_repo_paths(backend.as_ref()).await?;
    let mut configs = Vec::with_capacity(repo_uids.len());
    for uid in &repo_uids {
        let (config, _etag) = storage::settings::get_repo_config(backend.as_ref(), uid.as_str(), &cipher).await?;
        configs.push((uid.clone(), config));
    }

    let visible_now: HashSet<Uid> = storage::settings::list_visible_repo_uids(backend.as_ref())
        .await?
        .into_iter()
        .collect();
    let mut owned_now: HashMap<String, HashSet<Uid>> = HashMap::new();

    let mut needs_owner: Vec<(Uid, OwnerRef)> = Vec::new();
    let mut needs_visible: Vec<Uid> = Vec::new();
    let mut unset_owners = 0usize;
    for (uid, config) in &configs {
        if let Some(owner) = &config.owner {
            if owner_is_unset(owner) {
                unset_owners += 1;
                continue;
            }
            let key = owner_dedup_key(owner);
            if !owned_now.contains_key(&key) {
                let listed = storage::settings::list_owned_repo_uids(backend.as_ref(), owner).await?;
                owned_now.insert(key.clone(), listed.into_iter().collect());
            }
            if !owned_now[&key].contains(uid) {
                needs_owner.push((uid.clone(), owner.clone()));
            }
        }
        if config.visibility != gitquarry_core::storage::entities::Visibility::Private && !visible_now.contains(uid) {
            needs_visible.push(uid.clone());
        }
    }

    let users = access::list_all_users(&backend, &cipher).await?;
    let mut permission_now: HashMap<Uid, HashSet<Uid>> = HashMap::new();
    let mut needs_permission: Vec<(Uid, Uid)> = Vec::new(); // (repo_uid, user_uid)
    for user in &users {
        for repo_uid in user.repos.keys() {
            if !permission_now.contains_key(repo_uid) {
                let listed = access::list_repo_permission_uids(backend.as_ref(), repo_uid).await?;
                permission_now.insert(repo_uid.clone(), listed.into_iter().collect());
            }
            if !permission_now[repo_uid].contains(&user.uid) {
                needs_permission.push((repo_uid.clone(), user.uid.clone()));
            }
        }
    }

    if unset_owners > 0 {
        println!(
            "gitquarry: warning: {unset_owners} repo(s) have an 'owner' with an empty uid (the \"unassigned\" \
             sentinel, not a real owner) -- not indexed, review by hand from each one's settings tab."
        );
    }

    if needs_owner.is_empty() && needs_visible.is_empty() && needs_permission.is_empty() {
        println!(
            "gitquarry: {} repo(s) and {} user(s) checked, indexes already up to date.",
            configs.len(),
            users.len()
        );
        return Ok(());
    }

    println!(
        "gitquarry: {} repo(s) and {} user(s) checked -- {} owner-index entries, {} visible-index, and {} \
         permission-index entries missing.",
        configs.len(),
        users.len(),
        needs_owner.len(),
        needs_visible.len(),
        needs_permission.len()
    );

    if !apply {
        println!("(dry-run -- run again with --apply to write the indexes)");
        return Ok(());
    }

    for (uid, owner) in &needs_owner {
        storage::settings::add_owner_index(backend.as_ref(), &cipher, owner, uid).await;
    }
    for uid in &needs_visible {
        storage::settings::set_visible_index(backend.as_ref(), &cipher, uid, true).await;
    }
    for (repo_uid, user_uid) in &needs_permission {
        access::add_repo_permission_index(backend.as_ref(), &cipher, repo_uid, user_uid).await;
    }
    println!("gitquarry: indexes rebuilt.");
    Ok(())
}
