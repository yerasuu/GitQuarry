//! The `pre-receive` hook [`super::ensure_bare_repo`] installs on every
//! local mirror: invoked by `git` itself (as `gitquarry pre-receive-hook`,
//! see `main::run_hook`) inside the mirror's own process, not part of the
//! `ssh://`/http network servers. Checks that the repo isn't archived, that
//! no protected branch/tag is force-pushed, moved, or deleted by a
//! non-owner, that no ref moved in S3 since the last sync (avoids stomping
//! a concurrent push from another host), uploads new packs, and updates
//! `refs.json` with CAS.

use crate::auth::access;
use crate::crypto::Cipher;
use crate::storage::remote::Remote;
use crate::storage::{Backend, Cas};
use crate::{config, git};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tracing::warn;

/// Zero sha: git uses it in the hook for "this ref didn't exist" (creation)
/// or "doesn't exist anymore" (deletion).
const ZERO_SHA_LEN: [usize; 2] = [40, 64];

#[cfg(any(test, feature = "test-util"))]
pub fn is_zero_sha(s: &str) -> bool {
    ZERO_SHA_LEN.contains(&s.len()) && s.bytes().all(|b| b == b'0')
}
#[cfg(not(any(test, feature = "test-util")))]
fn is_zero_sha(s: &str) -> bool {
    ZERO_SHA_LEN.contains(&s.len()) && s.bytes().all(|b| b == b'0')
}

#[cfg(any(test, feature = "test-util"))]
pub fn zero_sha_like(sample: &str) -> String {
    "0".repeat(if sample.len() == 64 { 64 } else { 40 })
}
#[cfg(not(any(test, feature = "test-util")))]
fn zero_sha_like(sample: &str) -> String {
    "0".repeat(if sample.len() == 64 { 64 } else { 40 })
}

/// One line of the hook's stdin: `<old-sha> <new-sha> <refname>`.
#[cfg(any(test, feature = "test-util"))]
pub struct RefUpdate {
    pub refname: String,
    old: String,
    pub new: String,
}
#[cfg(not(any(test, feature = "test-util")))]
struct RefUpdate {
    refname: String,
    old: String,
    new: String,
}

#[cfg(any(test, feature = "test-util"))]
pub fn parse_hook_input(input: &str) -> Vec<RefUpdate> {
    input
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            Some(RefUpdate {
                old: it.next()?.to_string(),
                new: it.next()?.to_string(),
                refname: it.next()?.to_string(),
            })
        })
        .collect()
}
#[cfg(not(any(test, feature = "test-util")))]
fn parse_hook_input(input: &str) -> Vec<RefUpdate> {
    input
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            Some(RefUpdate {
                old: it.next()?.to_string(),
                new: it.next()?.to_string(),
                refname: it.next()?.to_string(),
            })
        })
        .collect()
}

/// `remote` alone (per-repo-prefixed keys) can't reach `access.json` — that
/// lives at the bucket root, not under any repo's prefix — so the raw
/// pieces are kept alongside it for the protected-branch check below, which
/// needs both.
struct HookCtx {
    remote: Remote,
    backend: Arc<dyn Backend>,
    cipher: Arc<Cipher>,
}

async fn build_remote(bucket: &str, repo_prefix: String, config_path: &Path) -> Result<HookCtx> {
    let cfg = config::Config::load_from(config_path)?;
    let bucket_cfg = cfg.bucket(bucket)?;
    let backend = crate::storage::build_backend(bucket_cfg)?;
    let cipher =
        Arc::new(crate::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?);
    let remote = Remote::new(Arc::clone(&backend), repo_prefix, Arc::clone(&cipher));
    Ok(HookCtx {
        remote,
        backend,
        cipher,
    })
}

/// Whether `actor` may push directly to a protected branch: a global admin,
/// the repo's own owner (user-owned repo), or a `Permission::Owner` member
/// of the group that owns it — the same "who runs this namespace" set
/// `auth::web::groups`/`webui::repo_settings` already use, never just plain
/// `Permission::Developer` (which most group members have). Fetches only the
/// actor's own record, plus (if the owner names a group) that one group's,
/// never the rest of the bucket's users/groups. Split from
/// [`can_push_protected_decision`] so the actual decision logic stays
/// unit-testable without a live per-entity fetch.
async fn can_push_protected(
    backend: &dyn Backend,
    cipher: &Cipher,
    owner: Option<&crate::storage::entities::OwnerRef>,
    actor: Option<&str>,
) -> Result<bool> {
    let Some(actor) = actor else { return Ok(false) };
    let user = access::get_user(backend, cipher, actor).await?.0;
    let owner_group = match owner {
        Some(crate::storage::entities::OwnerRef::Group(uid)) => access::resolve_group_uid(backend, cipher, uid).await?,
        _ => None,
    };
    Ok(can_push_protected_decision(user.as_ref(), owner, owner_group.as_ref()))
}

#[cfg(any(test, feature = "test-util"))]
pub fn can_push_protected_decision(
    user: Option<&crate::auth::entities::User>,
    owner: Option<&crate::storage::entities::OwnerRef>,
    owner_group: Option<&crate::auth::entities::Group>,
) -> bool {
    use crate::storage::entities::OwnerRef;
    if user.is_some_and(|u| u.is_admin) {
        return true;
    }
    match owner {
        Some(OwnerRef::User(owner_uid)) => user.is_some_and(|u| &u.uid == owner_uid),
        Some(OwnerRef::Group(_)) => user.is_some_and(|u| {
            owner_group.is_some_and(|g| g.role_of(&u.uid) >= Some(crate::auth::entities::Permission::Owner))
        }),
        None => false,
    }
}
#[cfg(not(any(test, feature = "test-util")))]
fn can_push_protected_decision(
    user: Option<&crate::auth::entities::User>,
    owner: Option<&crate::storage::entities::OwnerRef>,
    owner_group: Option<&crate::auth::entities::Group>,
) -> bool {
    use crate::storage::entities::OwnerRef;
    if user.is_some_and(|u| u.is_admin) {
        return true;
    }
    match owner {
        Some(OwnerRef::User(owner_uid)) => user.is_some_and(|u| &u.uid == owner_uid),
        Some(OwnerRef::Group(_)) => user.is_some_and(|u| {
            owner_group.is_some_and(|g| g.role_of(&u.uid) >= Some(crate::auth::entities::Permission::Owner))
        }),
        None => false,
    }
}

#[cfg(any(test, feature = "test-util"))]
pub fn touched_refs<'a>(
    updates: &'a [RefUpdate],
    ref_prefix: &str,
    protected: &std::collections::BTreeSet<String>,
) -> Vec<&'a RefUpdate> {
    updates
        .iter()
        .filter(|u| {
            u.refname
                .strip_prefix(ref_prefix)
                .is_some_and(|short| protected.contains(short))
        })
        .collect()
}
#[cfg(not(any(test, feature = "test-util")))]
fn touched_refs<'a>(
    updates: &'a [RefUpdate],
    ref_prefix: &str,
    protected: &std::collections::BTreeSet<String>,
) -> Vec<&'a RefUpdate> {
    updates
        .iter()
        .filter(|u| {
            u.refname
                .strip_prefix(ref_prefix)
                .is_some_and(|short| protected.contains(short))
        })
        .collect()
}

/// Checks one batch of already-filtered protected-ref updates (all
/// branches, or all tags — never mixed, so the error message's `kind_label`
/// is right). `allow_fast_forward`: branches may still move forward
/// (`is_ancestor` check); tags never move at all once protected, so any
/// change to an existing one is rejected outright, not just a non-ancestor
/// one. Returns the first rejection reason, if any.
#[cfg(any(test, feature = "test-util"))]
pub async fn check_protected(
    git_dir: &Path,
    touched: &[&RefUpdate],
    allowed: bool,
    allow_fast_forward: bool,
    kind_label: &str,
) -> Result<Option<String>> {
    for update in touched {
        let short = update.refname.rsplit('/').next().unwrap_or(&update.refname);
        if !allowed {
            return Ok(Some(format!(
                "{kind_label} '{short}': only an owner may push directly."
            )));
        }
        if is_zero_sha(&update.new) {
            return Ok(Some(format!("{kind_label} '{short}': can't be deleted.")));
        }
        if is_zero_sha(&update.old) {
            continue; // creation, not a move — nothing to protect against yet
        }
        let rejected = if allow_fast_forward {
            !git::is_ancestor(git_dir, &update.old, &update.new).await?
        } else {
            update.new != update.old
        };
        if rejected {
            return Ok(Some(format!("{kind_label} '{short}': can't be moved/force-pushed.")));
        }
    }
    Ok(None)
}
#[cfg(not(any(test, feature = "test-util")))]
async fn check_protected(
    git_dir: &Path,
    touched: &[&RefUpdate],
    allowed: bool,
    allow_fast_forward: bool,
    kind_label: &str,
) -> Result<Option<String>> {
    for update in touched {
        let short = update.refname.rsplit('/').next().unwrap_or(&update.refname);
        if !allowed {
            return Ok(Some(format!(
                "{kind_label} '{short}': only an owner may push directly."
            )));
        }
        if is_zero_sha(&update.new) {
            return Ok(Some(format!("{kind_label} '{short}': can't be deleted.")));
        }
        if is_zero_sha(&update.old) {
            continue; // creation, not a move — nothing to protect against yet
        }
        let rejected = if allow_fast_forward {
            !git::is_ancestor(git_dir, &update.old, &update.new).await?
        } else {
            update.new != update.old
        };
        if rejected {
            return Ok(Some(format!("{kind_label} '{short}': can't be moved/force-pushed.")));
        }
    }
    Ok(None)
}

/// Checks that every newly-introduced commit on a touched
/// `require_signed_commits` branch (see `storage::entities::RepoConfig`'s
/// own doc comment) verifies against a registered account that currently
/// has at least `Permission::Developer` on this repo — a verified-but-
/// unauthorized signer (an ex-contributor whose key is still registered,
/// say) doesn't satisfy the requirement: the point is proving *this push*
/// came from someone allowed to make it, not merely that some account's key
/// signed it at some point. `have_tips` is every ref's sha *before* this
/// push (the manifest fetched earlier in [`run_pre_receive_hook`]) — what
/// bounds [`git::new_commit_shas`] to commits this push actually
/// introduces, not a touched branch's entire history.
async fn check_signed_commits(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    git_dir: &Path,
    repo_full_name: &str,
    touched: &[&RefUpdate],
    have_tips: &[String],
) -> Result<Option<String>> {
    for update in touched {
        if is_zero_sha(&update.new) {
            continue; // deletion, nothing new to check
        }
        for sha in git::new_commit_shas(git_dir, &update.new, have_tips).await? {
            // Shared with the inbound-mirror gate (`super::policy`), which
            // has to answer this exact question for a ref that never went
            // through a push at all.
            if !super::policy::commit_signature_authorized(backend, cipher, git_dir, repo_full_name, &sha).await? {
                let short = update.refname.rsplit('/').next().unwrap_or(&update.refname);
                return Ok(Some(format!(
                    "branch '{short}' requires signed commits: {} isn't signed by a registered key with push access.",
                    crate::validate::short_sha(&sha)
                )));
            }
        }
    }
    Ok(None)
}

/// `pre-receive` hook logic: checks that no ref moved in S3 since the last
/// sync (avoids stomping a concurrent push from another host), uploads new
/// packs, and updates refs.json with CAS. Returns true if the push is accepted.
pub async fn run_pre_receive_hook() -> Result<bool> {
    let bucket =
        std::env::var("GITQUARRY_BUCKET").context("GITQUARRY_BUCKET not set (this hook is invoked by gitquarry)")?;
    // The repo's S3 prefix is keyed by uid, not by its (rename-able) display
    // name — resolved once by the parent process (`AppContext::resolve_repo`/
    // `create_repo`) and handed down here directly, rather than this
    // subprocess re-resolving `GITQUARRY_REPO` through the repo-name index
    // itself (see `mirror::spawn_git_subcommand`'s doc comment for why that
    // would be a second, racy lookup).
    let repo_uid = std::env::var("GITQUARRY_REPO_UID")
        .context("GITQUARRY_REPO_UID not set (this hook is invoked by gitquarry)")?;
    let config_path =
        std::env::var("GITQUARRY_CONFIG").context("GITQUARRY_CONFIG not set (this hook is invoked by gitquarry)")?;
    // git always sets GIT_DIR for hooks (bare repo -> the local mirror).
    let git_dir = std::env::var("GIT_DIR").context("GIT_DIR not set (should come from git)")?;
    let repo_prefix = format!("{}/{repo_uid}", crate::storage::REPOS_PREFIX);
    let hook_ctx = build_remote(&bucket, repo_prefix, Path::new(&config_path)).await?;
    let remote = &hook_ctx.remote;

    let (repo_config, _) = remote.get_repo_config().await?;
    if repo_config.deleted_at.is_some() {
        warn!("repo is deleted, rejecting pushes");
        return Ok(false);
    }
    if repo_config.archived {
        warn!("repo is archived, rejecting pushes");
        return Ok(false);
    }
    if let Some((used, limit)) =
        crate::auth::access::check_quota(hook_ctx.backend.as_ref(), &hook_ctx.cipher, repo_config.owner.as_ref())
            .await?
    {
        warn!(
            used_bytes = used,
            limit_bytes = limit,
            "push rejected: owner is over their storage quota (free up space or ask an admin to raise the limit)"
        );
        return Ok(false);
    }

    let mut input = String::new();
    tokio::io::stdin()
        .read_to_string(&mut input)
        .await
        .context("reading pre-receive hook stdin")?;

    let updates = parse_hook_input(&input);
    if updates.is_empty() {
        return Ok(true);
    }

    let (manifest, etag) = remote.get_manifest().await?;

    let branches_touched = touched_refs(&updates, "refs/heads/", &repo_config.protected_branches);
    let tags_touched = touched_refs(&updates, "refs/tags/", &repo_config.protected_tags);
    if !branches_touched.is_empty() || !tags_touched.is_empty() {
        let actor = std::env::var("GITQUARRY_ACTOR").ok();
        let allowed = can_push_protected(
            hook_ctx.backend.as_ref(),
            &hook_ctx.cipher,
            repo_config.owner.as_ref(),
            actor.as_deref(),
        )
        .await?;
        // Branches allow a fast-forward; tags never move once protected —
        // there's no such thing as a "fast-forward" re-tag, any change to
        // an existing one is a rewrite of what that tag meant.
        if let Some(msg) = check_protected(
            Path::new(&git_dir),
            &branches_touched,
            allowed,
            true,
            "protected branch",
        )
        .await?
        {
            warn!("{msg}");
            return Ok(false);
        }
        if let Some(msg) = check_protected(Path::new(&git_dir), &tags_touched, allowed, false, "protected tag").await? {
            warn!("{msg}");
            return Ok(false);
        }
    }

    let signed_required_touched = touched_refs(&updates, "refs/heads/", &repo_config.require_signed_commits);
    if !signed_required_touched.is_empty() {
        let have_tips: Vec<String> = manifest.refs.values().cloned().collect();
        if let Some(msg) = check_signed_commits(
            &hook_ctx.backend,
            &hook_ctx.cipher,
            Path::new(&git_dir),
            &repo_config.full_name,
            &signed_required_touched,
            &have_tips,
        )
        .await?
        {
            warn!("{msg}");
            return Ok(false);
        }
    }

    for update in &updates {
        let manifest_sha = manifest
            .refs
            .get(&update.refname)
            .cloned()
            .unwrap_or_else(|| zero_sha_like(&update.new));
        if manifest_sha != update.old {
            warn!(
                refname = %update.refname,
                expected = %update.old,
                current = %manifest_sha,
                "ref changed in S3 since the last fetch, fetch and retry"
            );
            return Ok(false);
        }
    }

    // Checked against every ref update before any pack gets uploaded: doing
    // this mid-upload-loop could leave earlier refs' packs already written
    // to S3 by the time a later ref fails the check.
    if let Some(raw_limit) = &repo_config.lfs_max_blob_size {
        let max_size = crate::mirror::gc::parse_size(raw_limit).context("invalid lfs_max_blob_size in repo config")?;
        let mut check_tips: Vec<String> = manifest.refs.values().cloned().collect();
        for update in &updates {
            if is_zero_sha(&update.new) {
                continue;
            }
            let oversized = git::oversized_new_blobs(Path::new(&git_dir), &update.new, &check_tips, max_size).await?;
            if !oversized.is_empty() {
                warn!(limit = %raw_limit, "push rejected, blob(s) over the limit without LFS");
                for (path, size) in &oversized {
                    warn!(%path, size, "oversized blob");
                }
                warn!("track these with `git lfs track` and re-commit before pushing");
                return Ok(false);
            }
            check_tips.push(update.new.clone());
        }
    }

    let mut new_manifest = manifest.clone();
    let mut have_tips: Vec<String> = manifest.refs.values().cloned().collect();
    let tmp_dir = git::scratch_dir(Path::new(&git_dir))?;
    let mut uploaded_bytes: u64 = 0;

    for update in &updates {
        if is_zero_sha(&update.new) {
            new_manifest.refs.remove(&update.refname);
            if new_manifest.head.as_deref() == Some(update.refname.as_str()) {
                new_manifest.head = None;
            }
            continue;
        }
        if let Some(pack) =
            git::pack_objects_for_push(Path::new(&git_dir), &update.new, &have_tips, tmp_dir.path()).await?
        {
            // Best-effort quota accounting: a metadata read failing here
            // shouldn't fail the push itself, same tolerance
            // `storage::settings::bump_pack_bytes` (called below) already
            // has for its own write half.
            if let Ok(meta) = tokio::fs::metadata(&pack.pack_path).await {
                uploaded_bytes = uploaded_bytes.saturating_add(meta.len());
            }
            remote
                .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
                .await?;
        }
        new_manifest.refs.insert(update.refname.clone(), update.new.clone());
        if new_manifest.head.is_none() {
            new_manifest.head = Some(update.refname.clone());
        }
        have_tips.push(update.new.clone());
    }

    match remote.put_manifest(&new_manifest, etag.as_deref()).await? {
        Cas::Ok => {
            let uid = crate::storage::uid::Uid::from(repo_uid.clone());
            if uploaded_bytes > 0 {
                crate::storage::settings::bump_pack_bytes(
                    hook_ctx.backend.as_ref(),
                    &hook_ctx.cipher,
                    &uid,
                    uploaded_bytes,
                )
                .await;
            }
            // Only the default branch moves the "last updated" stamp: a
            // push that only touches a feature branch or a tag isn't code
            // landing on the repo's main line, which is what the home
            // page's "updated X ago" is meant to answer.
            if let Some(default_update) = updates
                .iter()
                .find(|u| new_manifest.head.as_deref() == Some(u.refname.as_str()))
            {
                // A brand-new default branch has no `old` to range from —
                // every commit reachable from `new` is new to this repo.
                let range = if is_zero_sha(&default_update.old) {
                    default_update.new.clone()
                } else {
                    format!("{}..{}", default_update.old, default_update.new)
                };
                let commits = git::rev_list_count(Path::new(&git_dir), &range).await.unwrap_or(0);
                let branch = default_update
                    .refname
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&default_update.refname);
                crate::storage::settings::touch_last_push(
                    hook_ctx.backend.as_ref(),
                    &hook_ctx.cipher,
                    &uid,
                    commits.try_into().unwrap_or(u32::MAX),
                    branch,
                )
                .await;
            }
            // The push itself never triggers an external-mirror sync
            // directly — this hook process is short-lived (git kills it as
            // soon as it exits), so it can't hold a long-running task past
            // its own return the way an in-process `tokio::spawn` inside
            // the main server can. Whatever's wired into `ctx.mirror_sync`
            // instead picks this repo up the next time anyone reads or
            // writes it (`mirror::sync_external_before_serving`, called
            // from `git::ssh`/`git::http` before every upload-pack/
            // receive-pack) or on the periodic sweep `gitquarry-server`
            // spawns in-process at startup — see
            // `docs/mirror-external-extraction-concept.md`.
            Ok(true)
        }
        Cas::Conflict => {
            warn!("conflict updating refs.json in S3 (another push won the race), retry");
            Ok(false)
        }
    }
}
