//! Every mutation a [`crate::storage::entities::RepoConfig`] or a repo's
//! own identity (name claim, manifest, physical storage) goes through —
//! the single place `create`/`rename`/`delete`/`fork`/permission-grant
//! logic lives, so `webui::repo_new`/`webui::fork`/`webui::repo_settings`/
//! `auth::web::favorites`/`git::ssh` all call the same code instead of each
//! re-implementing the same multi-step orchestration (which is exactly how
//! `git::ssh`'s implicit-create-on-push ended up skipping `RepoConfig`
//! entirely — see [`create`]'s doc comment).
//!
//! Transport-agnostic on purpose: no `gitquarry_webui::webui::AppError`, no CSRF/
//! session/actor concerns, no audit logging — those stay in whichever
//! caller has an HTTP request or an authenticated actor in hand. Errors are
//! plain `anyhow::Error` with the same user-facing Spanish messages the web
//! handlers already showed; callers that need a specific HTTP status map
//! them the same way `gitquarry_webui::webui::AppError::bad_request` already does.
//! Where there's one well-defined "expected" failure (name already taken,
//! nothing to resolve), the shape is `Result<Option<T>>` — same convention
//! [`crate::app::AppContext::resolve_repo`]/`create_repo` already use, not
//! a new one invented here.

use crate::app::{AppContext, ResolvedRepo};
use crate::auth::access;
use crate::auth::entities::Permission;
use crate::crypto::Cipher;
use crate::storage::entities::{Label, OwnerRef, RepoConfig, Visibility};
use crate::storage::remote::{Manifest, Remote};
use crate::storage::uid::Uid;
use crate::storage::{self, Backend, Cas};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a soft-deleted repo (`RepoConfig::deleted_at` set) survives
/// before `mirror::deleted_repo_sweep` calls [`purge`] on it. Fixed rather
/// than configurable — same "not worth a knob" call as the KDF params in
/// `gitquarry.toml`'s encryption section.
pub const DELETE_GRACE_SECS: u64 = 30 * 24 * 60 * 60;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Seconds left in the grace window for a repo whose `deleted_at` is
/// `deleted_at` — `0` once expired (the sweep hasn't necessarily run yet,
/// but nothing should present this as "still recoverable").
pub fn grace_seconds_remaining(deleted_at: u64) -> u64 {
    DELETE_GRACE_SECS.saturating_sub(now_secs().saturating_sub(deleted_at))
}

/// Resolves a namespace string (a form field naming a username or group) to
/// the [`OwnerRef`] it names — user tried first, then group, matching
/// [`access::reserve_namespace`]'s "never both" invariant. `None` if
/// neither exists. The one existence-only lookup shared by `/_new`'s
/// namespace field and the settings-tab owner-transfer/move forms — not by
/// `/_fork`, whose namespace field is authorization-gated (own username or
/// a group the caller manages), a different, session-dependent check that
/// belongs in `webui::fork` itself, not here.
pub async fn resolve_owner_ref(ctx: &AppContext, namespace: &str) -> Result<Option<OwnerRef>> {
    if let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, namespace)
        .await?
        .0
    {
        return Ok(Some(OwnerRef::User(user.uid)));
    }
    if let Some(group) = access::get_group(ctx.backend().as_ref(), &ctx.cipher, namespace)
        .await?
        .0
    {
        return Ok(Some(OwnerRef::Group(group.uid)));
    }
    Ok(None)
}

/// Creates a brand-new repo: mints a uid, claims `full_name` in the
/// repo-name index, writes an empty manifest, writes the initial
/// `RepoConfig`. `None` if `full_name` is already claimed by a live repo.
///
/// **The single place a repo is ever created** — used by `/_new` (full
/// `description`/`visibility` from the form) and by `git::ssh`'s
/// implicit-create-on-first-push (`description: None`,
/// `visibility: Visibility::default()`, `owner` resolved from the repo's
/// own namespace segment via [`resolve_owner_ref`] — `None` only for the
/// unusual case of a bare repo name with no namespace at all). Before this
/// existed, ssh's implicit path called `AppContext::create_repo` directly
/// and never wrote a `RepoConfig` at all — a repo pushed into existence had
/// no `.conf/config.json`, so `full_name` came back empty from
/// `get_repo_config`'s default-on-404, and the repo showed up as a blank
/// row on `/` and `/_profile/<user>` (both list by `RepoConfig::full_name`
/// now). Routing every creation path through here closes that structurally
/// — there is no second place left to forget the config write.
pub async fn create(
    ctx: &AppContext,
    full_name: &str,
    display_name: Option<String>,
    owner: Option<OwnerRef>,
    description: Option<String>,
    visibility: Visibility,
) -> Result<Option<ResolvedRepo>> {
    let Some(resolved) = ctx.create_repo(full_name).await? else {
        return Ok(None);
    };
    match resolved.remote.put_manifest(&Manifest::default(), None).await? {
        Cas::Ok => {}
        // Identity claim landed but someone else's identical create raced
        // the manifest write — treat it the same as "name taken".
        Cas::Conflict => return Ok(None),
    }
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: full_name.to_string(),
        display_name,
        description,
        owner,
        visibility,
        ..Default::default()
    };
    // Best-effort: the repo already exists (manifest just created) even if
    // this fails — worst case it starts with a still-mostly-empty config,
    // still editable from its own settings tab afterward. Matches the
    // tolerance every other config write in this module already accepts.
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await;
    if let Some(owner) = &config.owner {
        storage::settings::add_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner, &resolved.uid).await;
    }
    storage::settings::set_visible_index(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &resolved.uid,
        config.visibility != Visibility::Private,
    )
    .await;
    Ok(Some(resolved))
}

/// Relocates the repo — since physical storage is keyed by `resolved.uid`,
/// not by its display name, this is a pure metadata update: claim
/// `new_full_name` in the repo-name index for the *same* uid, rewrite
/// `full_name`/`owner` on the *same* `RepoConfig` object, release the old
/// name claim. No git data moves, nothing gets re-encrypted — unlike
/// [`fork`]'s `copy_all_to` (a genuine duplication), a rename has no reason
/// to touch a single pack. `None` if `new_full_name` is already claimed by
/// a live repo; `Err` if a concurrent edit to this repo's own config lost
/// the race (the name claim is rolled back in that case, so nothing is left
/// stranded pointing at a repo still called `current_full_name` everywhere
/// else).
pub async fn rename(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    current_full_name: &str,
    new_full_name: &str,
    new_owner: OwnerRef,
    new_display_name: Option<String>,
    locale: gitquarry_i18n::Locale,
) -> Result<Option<String>> {
    // Same rule `AppContext::create_repo` applies to a fresh repo: a repo
    // path and a group path share one URL space, so a rename can't move a
    // repo onto a name a group namespace already holds.
    if crate::auth::access::namespace_reserved(ctx.backend().as_ref(), &ctx.cipher, new_full_name).await? {
        return Ok(None);
    }
    let claimed = storage::settings::claim_repo_name_self_healing(
        ctx.backend().as_ref(),
        &ctx.cipher,
        new_full_name,
        &resolved.uid,
        |existing_uid| {
            let remote = ctx.remote_for_uid(existing_uid);
            async move { remote.exists().await }
        },
    )
    .await?;
    if !claimed {
        return Ok(None);
    }

    let (current_config, etag) = resolved.remote.get_repo_config().await?;
    let new_config = RepoConfig {
        full_name: new_full_name.to_string(),
        owner: Some(new_owner),
        display_name: new_display_name,
        ..current_config
    };
    match storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &new_config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?
    {
        Cas::Ok => {}
        Cas::Conflict => {
            storage::settings::release_repo_name(ctx.backend().as_ref(), new_full_name).await;
            bail!("{}", gitquarry_i18n::shared::cas_conflict(locale));
        }
    }
    storage::settings::release_repo_name(ctx.backend().as_ref(), current_full_name).await;
    if let Some(old_owner) = &current_config.owner {
        storage::settings::remove_owner_index(ctx.backend().as_ref(), &ctx.cipher, old_owner, &resolved.uid).await;
    }
    if let Some(owner) = &new_config.owner {
        storage::settings::add_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner, &resolved.uid).await;
    }
    Ok(Some(new_full_name.to_string()))
}

/// Soft-delete: stamps `deleted_at` and hides the repo from
/// `.conf/index/repo-visible` and `.conf/index/repo-deleted` gains an entry
/// (so `mirror::deleted_repo_sweep` can find it without scanning every repo
/// in the bucket). Everything else — name claim, owner index, permission
/// grants, the git data itself — stays untouched, so [`restore`] is a plain
/// undo and git ssh/http (which both reject any repo with `deleted_at` set,
/// see `git::ssh`/`git::http`) has nothing left to re-derive. The actual
/// irreversible wipe is [`purge`], run only after `DELETE_GRACE_SECS` has
/// elapsed.
pub async fn delete(ctx: &AppContext, resolved: &ResolvedRepo, _full_name: &str) -> Result<()> {
    set_deleted_at(ctx, resolved, true).await?;
    storage::settings::set_visible_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, false).await;
    storage::settings::set_deleted_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, true).await;
    Ok(())
}

/// Undoes [`delete`] within the grace window: clears `deleted_at`, drops
/// the repo from `.conf/index/repo-deleted`, and restores
/// `.conf/index/repo-visible` to whatever `visibility` already says (same
/// rule [`create`]/[`fork`] use, not unconditionally visible again).
pub async fn restore(ctx: &AppContext, resolved: &ResolvedRepo) -> Result<()> {
    let config = set_deleted_at(ctx, resolved, false).await?;
    storage::settings::set_deleted_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, false).await;
    storage::settings::set_visible_index(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &resolved.uid,
        config.visibility != Visibility::Private,
    )
    .await;
    Ok(())
}

/// Flips `deleted_at` with a fresh read per attempt and returns the config
/// that actually landed. `Err` if the repo is already in the requested
/// state or every attempt lost its CAS race — either way before
/// [`delete`]/[`restore`] touch an index, since an index updated for a
/// config write that never happened leaves the repo hidden but not deleted
/// (or out of the sweep's index while `deleted_at` stays set).
async fn set_deleted_at(ctx: &AppContext, resolved: &ResolvedRepo, deleted: bool) -> Result<RepoConfig> {
    const MAX_ATTEMPTS: u32 = 5;
    for attempt in 1..=MAX_ATTEMPTS {
        // A `Conflict` drops the cached copy, so this re-read sees the
        // writer that beat us rather than the same stale etag again.
        let (current, etag) = resolved.remote.get_repo_config().await?;
        match (deleted, current.deleted_at.is_some()) {
            (true, true) => bail!("repo already deleted"),
            (false, false) => bail!("repo isn't deleted"),
            _ => {}
        }
        let config = RepoConfig {
            deleted_at: deleted.then(now_secs),
            ..current
        };
        match storage::settings::put_repo_config(
            ctx.backend().as_ref(),
            resolved.uid.as_str(),
            &config,
            etag.as_deref(),
            &ctx.cipher,
        )
        .await?
        {
            Cas::Ok => return Ok(config),
            Cas::Conflict => storage::cas_retry_backoff(attempt).await,
        }
    }
    bail!("repo config kept changing concurrently; deletion state left unchanged")
}

/// Irreversible: wipes every object under the repo's uid prefix and
/// releases its name claim so a future `create` under the same name
/// doesn't pay a degraded conflict-then-reclaim round trip. Normally only
/// reached once a soft-[`delete`]'s grace window has actually elapsed
/// (`mirror::deleted_repo_sweep`) — or, skipping that wait, an instance
/// admin's "force purge now" (`webui::repo_settings`'s `purge_repo` action,
/// gated on `User::is_admin` since it throws away the recovery window
/// [`delete`] exists to provide). `Err` if the repo isn't currently
/// soft-deleted — this is never a way to skip [`delete`] itself.
pub async fn purge(ctx: &AppContext, resolved: &ResolvedRepo, full_name: &str) -> Result<()> {
    // Read before wiping: `delete_all` takes `config.json` with it, and the
    // owner-index cleanup below needs to know which owner to unclaim from.
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if config.deleted_at.is_none() {
        bail!("repo isn't deleted");
    }
    resolved.remote.delete_all().await?;
    storage::settings::release_repo_name(ctx.backend().as_ref(), full_name).await;
    if let Some(owner) = &config.owner {
        storage::settings::remove_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner, &resolved.uid).await;
    }
    storage::settings::set_visible_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, false).await;
    storage::settings::set_deleted_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, false).await;
    // `delete_all` above took the repo's own `mirrors/` objects with it, but
    // this index lives outside the repo prefix: left behind, it makes the
    // external-mirror sweep resolve and list a uid that no longer exists,
    // once per tick, forever — one leaked entry per purged repo.
    let _ =
        storage::settings::set_mirrors_enabled_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, false).await;
    access::clear_repo_permission_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid).await?;
    Ok(())
}

/// Collections a repo duplication (`fork`/`create_from_template`, both
/// built on `Remote::copy_all_to`, which copies every key it finds) must
/// strip from the destination.
///
/// `mirrors/` is the security-relevant one: an `ExternalMirror` holds a
/// *usable* forge credential (reversible by necessity — the sync loop
/// re-authenticates with it on every tick), so inheriting it would hand
/// whoever forked a public repo both the ability to push that repo's refs
/// to the original owner's GitHub and — by repointing `remote_url` at a
/// host they control, which needs no token to do since a blank token field
/// means "keep the stored one" — the credential itself.
///
/// `merges/`, `lfs-locks/` and `lfs-lock-paths/` aren't credentials, just wrong: a fork
/// starting life with the upstream's merge requests, review threads and
/// held LFS locks isn't what a fork is, and no forge behaves that way.
///
/// `publish/` is a publish mirror's rewritten repo and mapping: it belongs
/// to a mirror of the source, and the copy has none.
const NOT_INHERITED_BY_A_COPY: [&str; 5] = ["mirrors/", "merges/", "lfs-locks/", "lfs-lock-paths/", "publish/"];

/// Strips [`NOT_INHERITED_BY_A_COPY`] from a freshly copied repo. Runs
/// *after* the copy rather than filtering inside `copy_all_to`: that
/// primitive's contract is "every object, re-encrypted under the
/// destination's key", and which collections a *duplication* should carry
/// is a domain decision, not a storage one.
async fn strip_uninheritable(dest: &ResolvedRepo) -> Result<()> {
    for sub_prefix in NOT_INHERITED_BY_A_COPY {
        dest.remote.delete_under(sub_prefix).await?;
    }
    Ok(())
}

/// Duplicates `source` into a brand-new repo under `dest_owner`, uid and
/// all — `copy_all_to` (a genuine duplication, unlike [`rename`]'s
/// metadata-only move) plus a fresh `RepoConfig` (`forked_from` set to the
/// source's uid, counters reset, `archived`/protections *not* inherited —
/// a fork of an archived repo isn't itself read-only, and branch/tag
/// protection is the new owner's own call to make). Bumps the source's own
/// `fork_count`. `None` if `dest_full_name` is already claimed by a live
/// repo. Same-bucket only — federation layer 2's cross-instance equivalent
/// is [`crate::domain::remote_fork::seed_from_remote`], kept in its own
/// module rather than here since it seeds from a peer-synced mirror instead
/// of this bucket's own `Remote::copy_all_to`.
pub async fn fork(
    ctx: &AppContext,
    source: &ResolvedRepo,
    dest_full_name: &str,
    dest_display_name: Option<String>,
    dest_owner: OwnerRef,
    locale: gitquarry_i18n::Locale,
) -> Result<Option<ResolvedRepo>> {
    let (source_config, _etag) = source.remote.get_repo_config().await?;
    let Some(dest) = ctx.create_repo(dest_full_name).await? else {
        return Ok(None);
    };

    source.remote.copy_all_to(&dest.remote).await?;
    // Only proceed once the destination is confirmed to actually be there —
    // a copy that silently did nothing must never end with a fork that
    // looks created but isn't.
    if !dest.remote.exists().await? {
        bail!("{}", gitquarry_i18n::shared::repo::copy_seems_to_have_failed(locale));
    }
    strip_uninheritable(&dest).await?;

    let new_config = RepoConfig {
        uid: dest.uid.clone(),
        full_name: dest_full_name.to_string(),
        display_name: dest_display_name,
        description: source_config.description.clone(),
        owner: Some(dest_owner),
        visibility: source_config.visibility,
        archived: false,
        deleted_at: None,
        // A byte-for-byte copy landing right now is itself a content change
        // for the new repo, same as a push or a mirror sync would be.
        last_push: Some(now_secs()),
        last_push_commits: None,
        last_push_branch: None,
        // A real byte-for-byte copy (`copy_all_to` just above), unlike
        // `domain::remote_fork`'s peer-sync case — the new owner's quota
        // accounting should reflect what they actually just received, not
        // start at zero and undercount until the next push.
        pack_bytes: source_config.pack_bytes,
        lfs_bytes: source_config.lfs_bytes,
        required_reviewers: Default::default(),
        is_template: false,
        protected_branches: Default::default(),
        protected_tags: Default::default(),
        require_signed_commits: Default::default(),
        lfs_max_blob_size: Default::default(),
        topics: source_config.topics.clone(),
        forked_from: Some(source.uid.clone()),
        forked_from_peer: None,
        fork_count: 0,
        star_count: 0,
        watcher_count: 0,
        // A local fork copies the source's git data directly (just above),
        // so its language mix is identical at fork time -- no need to wait
        // for this fork's own first push to know it, unlike
        // `domain::remote_fork` (copying from a peer, no local guarantee
        // the source's own cached value is even trustworthy).
        primary_language: source_config.primary_language.clone(),
        // Same reasoning as `topics`/`primary_language` just above: a local
        // fork copies the source's git data directly, so inheriting its
        // label vocabulary is safe (unlike `domain::remote_fork`, where the
        // peer's label names have no guaranteed local meaning).
        labels: source_config.labels.clone(),
    };
    // `copy_all_to` just copied the source's own `config.json` to `dest`
    // too, so the CAS etag to write against is whatever that copy landed
    // with — not "must not exist", which would wrongly conflict. Best-
    // effort: the fork already succeeded either way; worst case it starts
    // with the copied-over (now stale) config, still editable afterward.
    let (_copied_config, copied_etag) = dest.remote.get_repo_config().await?;
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        new_config.uid.as_str(),
        &new_config,
        copied_etag.as_deref(),
        &ctx.cipher,
    )
    .await;
    if let Some(owner) = &new_config.owner {
        storage::settings::add_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner, &dest.uid).await;
    }
    storage::settings::set_visible_index(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &dest.uid,
        new_config.visibility != Visibility::Private,
    )
    .await;

    bump_fork_count(ctx, &source.uid).await;
    Ok(Some(dest))
}

/// Best-effort +1 on the source repo's own `fork_count` — read/modify/CAS-
/// write. A lost race under a concurrent fork just undercounts by one (see
/// the field's own doc comment); not worth a retry loop for a display
/// counter.
async fn bump_fork_count(ctx: &AppContext, source_uid: &Uid) {
    let Ok((mut config, etag)) =
        storage::settings::get_repo_config(ctx.backend().as_ref(), source_uid.as_str(), &ctx.cipher).await
    else {
        return;
    };
    config.fork_count += 1;
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        source_uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await;
}

/// Duplicates `source` into a brand-new, fully independent repo — same
/// `Remote::copy_all_to` mechanism [`fork`] uses, but with no `forked_from`
/// link and no `fork_count` bump: this repo's history starts here, it was
/// never "forked from" `source` in any sense a reader would expect that
/// label to mean. `Err` if `source` isn't marked [`RepoConfig::is_template`]
/// — creating from an arbitrary repo nobody opted into offering as a
/// starting point isn't what this is for (use [`fork`] instead). `None` if
/// `dest_full_name` is already claimed by a live repo.
pub async fn create_from_template(
    ctx: &AppContext,
    source: &ResolvedRepo,
    dest_full_name: &str,
    dest_display_name: Option<String>,
    dest_owner: OwnerRef,
    locale: gitquarry_i18n::Locale,
) -> Result<Option<ResolvedRepo>> {
    let (source_config, _etag) = source.remote.get_repo_config().await?;
    if !source_config.is_template {
        bail!("{}", gitquarry_i18n::shared::repo::repo_not_a_template(locale));
    }
    let Some(dest) = ctx.create_repo(dest_full_name).await? else {
        return Ok(None);
    };

    source.remote.copy_all_to(&dest.remote).await?;
    if !dest.remote.exists().await? {
        bail!("{}", gitquarry_i18n::shared::repo::copy_seems_to_have_failed(locale));
    }
    // A template is *published* to be copied by other people, so inheriting
    // the template author's mirror credential here would be worse than in
    // `fork`, not better.
    strip_uninheritable(&dest).await?;

    let new_config = RepoConfig {
        uid: dest.uid.clone(),
        full_name: dest_full_name.to_string(),
        display_name: dest_display_name,
        description: source_config.description.clone(),
        owner: Some(dest_owner),
        visibility: source_config.visibility,
        archived: false,
        deleted_at: None,
        // Same reasoning as `fork`'s own doc comment on this field: a
        // byte-for-byte copy landing right now is itself a content change.
        last_push: Some(now_secs()),
        last_push_commits: None,
        last_push_branch: None,
        // Same reasoning as `fork`'s own doc comment on this field: a real
        // byte-for-byte copy, so the new owner's quota should reflect it
        // from the start.
        pack_bytes: source_config.pack_bytes,
        lfs_bytes: source_config.lfs_bytes,
        required_reviewers: Default::default(),
        is_template: false,
        protected_branches: Default::default(),
        protected_tags: Default::default(),
        require_signed_commits: Default::default(),
        lfs_max_blob_size: Default::default(),
        topics: source_config.topics.clone(),
        forked_from: None,
        forked_from_peer: None,
        fork_count: 0,
        star_count: 0,
        watcher_count: 0,
        primary_language: source_config.primary_language.clone(),
        labels: source_config.labels.clone(),
    };
    let (_copied_config, copied_etag) = dest.remote.get_repo_config().await?;
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        new_config.uid.as_str(),
        &new_config,
        copied_etag.as_deref(),
        &ctx.cipher,
    )
    .await;
    if let Some(owner) = &new_config.owner {
        storage::settings::add_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner, &dest.uid).await;
    }
    storage::settings::set_visible_index(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &dest.uid,
        new_config.visibility != Visibility::Private,
    )
    .await;

    Ok(Some(dest))
}

/// `description`/`owner`/`visibility`/`archived`/`is_template` — the
/// settings-tab general form. Everything else on `RepoConfig` (topics,
/// protections, counters, identity) carries over untouched.
pub struct ConfPatch {
    pub description: Option<String>,
    pub owner: Option<OwnerRef>,
    pub visibility: Visibility,
    pub archived: bool,
    pub is_template: bool,
}

/// Re-reads the etag right before writing so a concurrent edit CAS-fails
/// instead of being silently overwritten. The `bool` half of the return is
/// whether `patch` actually differs from what was already stored — a
/// resubmit of an unchanged form still writes (same CAS-freshness reasoning
/// as any other save) but isn't real activity, so `webui::repo_settings`
/// skips the audit-log entry for it (`activity`'s own repo-activity feed
/// would otherwise be mostly "saved general settings" noise from people just
/// reopening and resaving the form).
pub async fn set_conf(ctx: &AppContext, resolved: &ResolvedRepo, patch: ConfPatch) -> Result<(Cas, bool)> {
    let (current, etag) = resolved.remote.get_repo_config().await?;
    let old_owner = current.owner.clone();
    let changed = patch.description != current.description
        || patch.owner != current.owner
        || patch.visibility != current.visibility
        || patch.archived != current.archived
        || patch.is_template != current.is_template;
    let config = RepoConfig {
        description: patch.description,
        owner: patch.owner,
        visibility: patch.visibility,
        archived: patch.archived,
        is_template: patch.is_template,
        ..current
    };
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    if cas == Cas::Ok {
        if old_owner != config.owner {
            if let Some(old) = &old_owner {
                storage::settings::remove_owner_index(ctx.backend().as_ref(), &ctx.cipher, old, &resolved.uid).await;
            }
            if let Some(new) = &config.owner {
                storage::settings::add_owner_index(ctx.backend().as_ref(), &ctx.cipher, new, &resolved.uid).await;
            }
        }
        storage::settings::set_visible_index(
            ctx.backend().as_ref(),
            &ctx.cipher,
            &resolved.uid,
            config.visibility != Visibility::Private,
        )
        .await;
    }
    Ok((cas, changed))
}

/// The default branch isn't cosmetic metadata: it's `Manifest::head` (not
/// `RepoConfig` at all), the same field `mirror::sync_bare_repo_from_s3`
/// sets a mirror's actual `HEAD` symref from — `git clone`/`ls-remote` over
/// any transport see the new default too, not just the web UI. `Err` if
/// `branch` isn't one of the repo's actual branches.
pub async fn set_default_branch(resolved: &ResolvedRepo, branch: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let (mut manifest, etag) = resolved.remote.get_manifest().await?;
    let target = format!("refs/heads/{branch}");
    if !manifest.refs.contains_key(&target) {
        bail!("{}", gitquarry_i18n::shared::repo::not_a_branch(locale, branch));
    }
    manifest.head = Some(target);
    resolved.remote.put_manifest(&manifest, etag.as_deref()).await
}

/// `Err` if `tag` already names a ref (branch or tag) in this repo — no
/// silent overwrite of someone else's ref from the web UI. `sha` is trusted
/// here: the caller (`webui::repo::tags`) already resolved and verified it
/// against the actual git data (via `git::browse::resolve_commit`) before
/// calling this, same division of labor as `domain::release::create`
/// trusting its `tag` param already passed `validate::validate_rev`.
pub async fn create_tag(resolved: &ResolvedRepo, tag: &str, sha: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let (mut manifest, etag) = resolved.remote.get_manifest().await?;
    let target = format!("refs/tags/{tag}");
    if manifest.refs.contains_key(&target) {
        bail!("{}", gitquarry_i18n::shared::repo::tag_already_exists(locale, tag));
    }
    manifest.refs.insert(target, sha.to_string());
    resolved.remote.put_manifest(&manifest, etag.as_deref()).await
}

/// `Err` if `tag` is protected — same "never moved, never deleted" invariant
/// `mirror::hook::check_protected` enforces on `git push tag :vX.Y` (a
/// protected tag can't be deleted there even by the owner, see that
/// function's doc comment), applied here too so the web UI can't do through
/// this what a direct push already can't — or if `tag` doesn't currently
/// exist.
pub async fn delete_tag(resolved: &ResolvedRepo, tag: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if config.protected_tags.contains(tag) {
        bail!(
            "{}",
            gitquarry_i18n::shared::repo::protected_tag_cannot_be_deleted(locale, tag)
        );
    }
    let (mut manifest, etag) = resolved.remote.get_manifest().await?;
    let target = format!("refs/tags/{tag}");
    if manifest.refs.remove(&target).is_none() {
        bail!("{}", gitquarry_i18n::shared::repo::not_a_tag(locale, tag));
    }
    resolved.remote.put_manifest(&manifest, etag.as_deref()).await
}

/// `Err` if `branch` is protected (same unconditional bar [`delete_tag`]
/// applies, see [`gitquarry_i18n::shared::repo::protected_branch_cannot_be_deleted`]'s
/// own doc comment on why no owner bypass), is the repo's current default
/// branch, or doesn't currently exist.
pub async fn delete_branch(resolved: &ResolvedRepo, branch: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if config.protected_branches.contains(branch) {
        bail!(
            "{}",
            gitquarry_i18n::shared::repo::protected_branch_cannot_be_deleted(locale, branch)
        );
    }
    let (mut manifest, etag) = resolved.remote.get_manifest().await?;
    let target = format!("refs/heads/{branch}");
    if manifest.head.as_deref() == Some(target.as_str()) {
        bail!(
            "{}",
            gitquarry_i18n::shared::repo::default_branch_cannot_be_deleted(locale, branch)
        );
    }
    if manifest.refs.remove(&target).is_none() {
        bail!("{}", gitquarry_i18n::shared::repo::not_a_branch(locale, branch));
    }
    resolved.remote.put_manifest(&manifest, etag.as_deref()).await
}

pub fn parse_csv_field(s: &str) -> BTreeSet<String> {
    s.split(',')
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .collect()
}

/// Protected branches reject force-push/delete and gate direct pushes to
/// owners-only — see `mirror::hook::can_push_protected` for the actual
/// enforcement, this only edits which branches count. `raw_csv` is
/// comma-separated; each name is checked against the repo's actual
/// branches so a typo can't silently "protect" one that doesn't exist.
/// Returns the parsed set alongside the CAS result so the caller can build
/// an audit-log detail string without a second fetch.
pub async fn set_protected_branches(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    raw_csv: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(Cas, BTreeSet<String>)> {
    let (mut config, etag) = resolved.remote.get_repo_config().await?;
    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    let valid: BTreeSet<&str> = manifest
        .refs
        .keys()
        .filter_map(|r| r.strip_prefix("refs/heads/"))
        .collect();
    let requested = parse_csv_field(raw_csv);
    for branch in &requested {
        if !valid.contains(branch.as_str()) {
            bail!("{}", gitquarry_i18n::shared::repo::not_a_branch(locale, branch));
        }
    }
    config.protected_branches = requested.clone();
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    Ok((cas, requested))
}

/// Same shape as [`set_protected_branches`] — a branch requiring GPG-signed
/// commits (enforced at `mirror::hook`, see `RepoConfig::require_signed_commits`'s
/// own doc comment) is an independent set from `protected_branches`, not a
/// modifier on it: a branch can be one, both, or neither.
pub async fn set_require_signed_commits(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    raw_csv: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(Cas, BTreeSet<String>)> {
    let (mut config, etag) = resolved.remote.get_repo_config().await?;
    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    let valid: BTreeSet<&str> = manifest
        .refs
        .keys()
        .filter_map(|r| r.strip_prefix("refs/heads/"))
        .collect();
    let requested = parse_csv_field(raw_csv);
    for branch in &requested {
        if !valid.contains(branch.as_str()) {
            bail!("{}", gitquarry_i18n::shared::repo::not_a_branch(locale, branch));
        }
    }
    config.require_signed_commits = requested.clone();
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    Ok((cas, requested))
}

/// Same idea as [`set_protected_branches`] but for tags — a tag is never
/// fast-forwarded, so once protected any move at all is rejected, not just
/// a non-ancestor one (see `mirror::hook::check_protected`'s
/// `allow_fast_forward`).
pub async fn set_protected_tags(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    raw_csv: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(Cas, BTreeSet<String>)> {
    let (mut config, etag) = resolved.remote.get_repo_config().await?;
    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    let valid: BTreeSet<&str> = manifest
        .refs
        .keys()
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .collect();
    let requested = parse_csv_field(raw_csv);
    for tag in &requested {
        if !valid.contains(tag.as_str()) {
            bail!("{}", gitquarry_i18n::shared::repo::not_a_tag(locale, tag));
        }
    }
    config.protected_tags = requested.clone();
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    Ok((cas, requested))
}

fn normalize_topic(raw: &str) -> Option<String> {
    let normalized: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(32)
        .collect();
    (!normalized.is_empty()).then_some(normalized)
}

const MAX_REPO_SLUG_LEN: usize = 100;

/// Derives a `sanitize_path`-safe local-name segment from a repo's free-text
/// `display_name` — unlike [`normalize_topic`], case is preserved (a repo
/// slug isn't a search facet, no reason to force it lowercase) and the
/// result is meant to round-trip back through `sanitize_path` unchanged, not
/// just be safe to drop into a link. Characters already in
/// `sanitize_path`'s allowed set (`[A-Za-z0-9._~-]`) pass through; anything
/// else (spaces, symbols, non-ASCII) becomes `-`, runs of `-` collapse to
/// one, and leading/trailing `-`/`.` are trimmed (a leading `-` would
/// otherwise look like a flag to `sanitize_path`, and to `git` itself
/// downstream). `None` if nothing survives — a name that's pure emoji/CJK/
/// punctuation has no safe slug to offer; the caller treats that as a
/// rejected name, same as an empty one.
pub fn slugify_repo_name(display_name: &str) -> Option<String> {
    let mut out = String::with_capacity(display_name.len());
    let mut last_was_dash = false;
    for c in display_name.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '~') {
            out.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '.');
    let capped = trimmed.get(..trimmed.len().min(MAX_REPO_SLUG_LEN)).unwrap_or(trimmed);
    let capped = capped.trim_matches(|c| c == '-' || c == '.');
    (!capped.is_empty()).then(|| capped.to_string())
}

/// Best-effort backfill for a repo that predates `RepoConfig::display_name`
/// (or whose slug has otherwise drifted from it): the current `full_name`'s
/// local segment becomes the public `display_name` (preserved exactly, even
/// if it has characters `sanitize_path` would now reject), and a clean slug
/// is derived from it. Never propagates an error — a cosmetic migration
/// failing must never break the listing page that triggered it (same
/// fail-quiet spirit `webui::repo::list_visible_repos`'s own doc comment
/// already applies to privacy checks); a lost race or a name collision on
/// the derived slug just leaves `config` as it was, retried on the next load.
///
/// Deliberately only called from the uid-based listing paths (home index,
/// profile, group pages) — not from `AppContext::resolve_repo` or any
/// ssh/http request path, since those resolve a repo by its *current*
/// `full_name` string, which a legacy repo with an invalid slug can never
/// again be reached by after `sanitize_path` tightened its charset. A
/// listing enumerates by uid instead, so it's the one place that can find
/// such a repo at all in order to fix it.
pub async fn backfill_slug(
    backend: Arc<dyn Backend>,
    cipher: Arc<Cipher>,
    uid: &Uid,
    config: RepoConfig,
    etag: Option<String>,
) -> RepoConfig {
    let (namespace, current_local) = config
        .full_name
        .rsplit_once('/')
        .unwrap_or(("", config.full_name.as_str()));
    if let Some(display_name) = &config.display_name
        && slugify_repo_name(display_name).as_deref() == Some(current_local)
    {
        return config;
    }
    let display_name = config.display_name.clone().unwrap_or_else(|| current_local.to_string());
    let Some(slug) = slugify_repo_name(&display_name) else {
        return config;
    };
    let namespace = namespace.to_string();

    if slug == current_local {
        // Path's already clean — only `display_name` was missing.
        let mut updated = config.clone();
        updated.display_name = Some(display_name);
        return match storage::settings::put_repo_config(
            backend.as_ref(),
            uid.as_str(),
            &updated,
            etag.as_deref(),
            &cipher,
        )
        .await
        {
            Ok(Cas::Ok) => updated,
            _ => config,
        };
    }

    let new_full_name = if namespace.is_empty() {
        slug
    } else {
        format!("{namespace}/{slug}")
    };
    let claimed = storage::settings::claim_repo_name_self_healing(backend.as_ref(), &cipher, &new_full_name, uid, {
        let backend = Arc::clone(&backend);
        let cipher = Arc::clone(&cipher);
        move |existing_uid| {
            let remote = Remote::new(
                Arc::clone(&backend),
                format!("{}/{existing_uid}", crate::storage::REPOS_PREFIX),
                Arc::clone(&cipher),
            );
            async move { remote.exists().await }
        }
    })
    .await;
    if !matches!(claimed, Ok(true)) {
        return config;
    }

    let mut updated = config.clone();
    updated.full_name = new_full_name.clone();
    updated.display_name = Some(display_name);
    match storage::settings::put_repo_config(backend.as_ref(), uid.as_str(), &updated, etag.as_deref(), &cipher).await {
        Ok(Cas::Ok) => {
            storage::settings::release_repo_name(backend.as_ref(), &config.full_name).await;
            updated
        }
        _ => {
            storage::settings::release_repo_name(backend.as_ref(), &new_full_name).await;
            config
        }
    }
}

/// Topics are cosmetic (discovery/categorization), so unlike
/// `set_protected_branches`/`set_protected_tags` there's nothing to
/// validate against — instead each one gets normalized to lowercase
/// `[a-z0-9-]`, capped at 32 chars, so it's always safe to drop straight
/// into a `?q=` link on the index page without extra escaping there. A
/// token that normalizes to nothing (e.g. all punctuation) is just
/// dropped, not an error — this is a convenience field, not one worth
/// rejecting typos on.
pub async fn set_topics(ctx: &AppContext, resolved: &ResolvedRepo, raw_csv: &str) -> Result<(Cas, BTreeSet<String>)> {
    let (mut config, etag) = resolved.remote.get_repo_config().await?;
    let topics: BTreeSet<String> = raw_csv.split(',').filter_map(normalize_topic).collect();
    config.topics = topics.clone();
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    Ok((cas, topics))
}

/// Parses `"alice, bob, ..."`, hard-validating each name resolves to a real
/// user — unlike [`set_topics`]'s silently-drop-the-bad-token tolerance, a
/// typo'd required reviewer would make every future MR on this repo
/// permanently unmergeable (`domain::merge_request::missing_required_reviewers`
/// has no way to know "nobody by that name can ever satisfy this"), so this
/// bails on the first name that doesn't resolve and names it, same as
/// `domain::issue::edit`'s assignee validation.
pub async fn set_required_reviewers(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    raw_csv: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(Cas, BTreeSet<String>)> {
    let mut required_reviewers = BTreeSet::new();
    for username in raw_csv.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let (user, _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?;
        if user.is_none() {
            bail!("{}", gitquarry_i18n::shared::repo::unknown_assignee(locale, username));
        }
        required_reviewers.insert(username.to_string());
    }
    let (mut config, etag) = resolved.remote.get_repo_config().await?;
    config.required_reviewers = required_reviewers.clone();
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    Ok((cas, required_reviewers))
}

/// Parses `"name:#color, name:#color, ..."`, hard-validating each color via
/// [`crate::validate::validate_hex_color`] — unlike [`normalize_topic`],
/// which silently drops a bad token, this bails on the first malformed
/// entry and names it: a broken color isn't cosmetic, it breaks the label
/// pill's rendering outright. A duplicate name (case-sensitive) just keeps
/// the last one, same "last write wins" a plain form re-submit would give
/// either way.
fn parse_labels_csv(raw_csv: &str) -> Result<Vec<Label>> {
    let mut by_name: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for entry in raw_csv.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((name, color)) = entry.rsplit_once(':') else {
            bail!("invalid label entry (expected name:#color): {entry}");
        };
        let name = name.trim();
        let color = color.trim();
        if name.is_empty() {
            bail!("invalid label entry (empty name): {entry}");
        }
        crate::validate::validate_hex_color(color)?;
        by_name.insert(name.to_string(), color.to_string());
    }
    Ok(by_name.into_iter().map(|(name, color)| Label { name, color }).collect())
}

/// Replaces the repo's whole label vocabulary in one CAS write — same
/// single-attempt, full-replace shape as [`set_topics`], parsing via
/// [`parse_labels_csv`].
pub async fn set_labels(ctx: &AppContext, resolved: &ResolvedRepo, raw_csv: &str) -> Result<(Cas, Vec<Label>)> {
    let labels = parse_labels_csv(raw_csv)?;

    let (mut config, etag) = resolved.remote.get_repo_config().await?;
    config.labels = labels.clone();
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    Ok((cas, labels))
}

/// `value: None` turns the feature off for this repo (the default); `Some`
/// is validated with `mirror::gc::parse_size` up front, same as
/// `settings::web`'s global `mirrors_max_size` field, so a typo shows up as
/// a form error instead of silently never rejecting anything. Enforcement
/// is `mirror::hook::run_pre_receive_hook`, not here — this only validates
/// and stores the string.
pub async fn set_lfs_max_blob_size(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    value: Option<String>,
) -> Result<(Cas, Option<String>)> {
    if let Some(raw) = &value {
        crate::mirror::gc::parse_size(raw)?;
    }
    let (mut config, etag) = resolved.remote.get_repo_config().await?;
    config.lfs_max_blob_size = value.clone();
    let cas = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await?;
    Ok((cas, value))
}

/// Sets (or, `perm: None`, clears) one user's explicit `Permission` for
/// `repo_uid`, leaving every other repo in their `repos` map and every
/// other user untouched — unlike `auth::domain::user`'s bulk repo-field parser,
/// this only ever touches the one (user, repo) cell a permission-grid row
/// represents. `Err` if `username` doesn't name an existing user.
pub async fn set_permission(
    ctx: &AppContext,
    repo_uid: &Uid,
    username: &str,
    perm: Option<Permission>,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (user, etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?;
    let mut user = user.with_context(|| gitquarry_i18n::shared::auth::user_not_found(locale, username))?;
    match perm {
        Some(p) => {
            user.repos.insert(repo_uid.clone(), p);
        }
        None => {
            user.repos.remove(repo_uid);
        }
    }
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok {
        match perm {
            Some(_) => {
                access::add_repo_permission_index(ctx.backend().as_ref(), &ctx.cipher, repo_uid, &user.uid).await
            }
            None => {
                access::remove_repo_permission_index(ctx.backend().as_ref(), &ctx.cipher, repo_uid, &user.uid).await
            }
        }
    }
    Ok(cas)
}

/// Toggles `username`'s star on `repo_uid`: read/modify/CAS-write the user
/// plus a best-effort ±1 on the repo's own `star_count`. `None` if the user
/// vanished between the caller's own session check and this call (rare —
/// `auth::web::favorites` re-reads the etag right before this for the same
/// reason). The visibility leak-guard (don't let a new star fingerprint a
/// private repo's existence) is a session/authorization concern and stays
/// in `auth::web::favorites`, checked before this is ever called.
pub async fn toggle_star(ctx: &AppContext, username: &str, repo_uid: &Uid) -> Result<Option<(Cas, bool)>> {
    // Bounded retry, unlike most CAS writes in this codebase (best-effort,
    // a lost race just drifts a display counter by one -- see
    // `bump_star_count`'s own doc comment): a lost race here would
    // silently drop the actual toggle a rapid double-click represents,
    // while `auth::web::favorites` still reports success back to the
    // browser as if it had taken. `starred` is a single user's own small
    // set, so a fresh read + retry is cheap and just resolves the race
    // instead of misreporting it.
    const MAX_ATTEMPTS: u8 = 5;
    for _ in 0..MAX_ATTEMPTS {
        let (Some(mut user), etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await? else {
            return Ok(None);
        };
        let now_starred = if user.starred.remove(repo_uid) {
            false
        } else {
            user.starred.insert(repo_uid.clone());
            true
        };
        let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
        if cas == Cas::Ok {
            bump_star_count(ctx, repo_uid, now_starred).await;
            return Ok(Some((Cas::Ok, now_starred)));
        }
    }
    Ok(Some((Cas::Conflict, false)))
}

/// Best-effort ±1 on the repo's own `star_count` — same pattern as
/// [`bump_fork_count`]. A lost race under concurrent star/unstar just
/// drifts by one (see the field's own doc comment); not worth a retry loop
/// for a display counter.
async fn bump_star_count(ctx: &AppContext, repo_uid: &Uid, now_starred: bool) {
    let Ok((mut config, etag)) =
        storage::settings::get_repo_config(ctx.backend().as_ref(), repo_uid.as_str(), &ctx.cipher).await
    else {
        return;
    };
    if now_starred {
        config.star_count = config.star_count.saturating_add(1);
    } else {
        config.star_count = config.star_count.saturating_sub(1);
    }
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        repo_uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await;
}

/// Best-effort refresh of `RepoConfig::primary_language` after a push —
/// same "read config, mutate, CAS put, drop the result" shape as
/// [`bump_star_count`], just called from `git::ssh`/`git::http` right after
/// a successful `receive-pack` instead of a star/watch toggle. Callers
/// `tokio::spawn` this detached (a full tree walk —
/// `git::browse::language_stats` — has no business delaying the push
/// response the client is already waiting on) which is exactly why this
/// takes its own fresh `ctx.locks` guard on `repo_uid` rather than
/// borrowing the caller's already-held `Mirror` guard: that one's lifetime
/// ends with the caller's own scope, which returns to the client long
/// before a detached task like this one is guaranteed to finish, and
/// reading a mirror after its lock's already been released (a concurrent
/// push or `mirror::gc` eviction could be touching it by then) is exactly
/// what the lock exists to prevent. Silently does nothing for an empty
/// repo (`resolve_head` returns `None`) or a repo with no
/// recognized-extension files (`language_stats` returns `[]`) — leaves
/// `primary_language` however it was, not forced back to `None`.
pub async fn update_primary_language(ctx: &AppContext, repo_uid: &Uid, git_dir: &std::path::Path) {
    let _guard = ctx.locks.lock(repo_uid.as_str()).await;
    let Ok(Some(head)) = crate::git::browse::resolve_head(git_dir).await else {
        return;
    };
    let Ok(stats) = crate::git::browse::language_stats(git_dir, &head).await else {
        return;
    };
    let Some((top_language, _bytes)) = stats.into_iter().max_by_key(|(_, bytes)| *bytes) else {
        return;
    };
    let Ok((mut config, etag)) =
        storage::settings::get_repo_config(ctx.backend().as_ref(), repo_uid.as_str(), &ctx.cipher).await
    else {
        return;
    };
    config.primary_language = Some(top_language);
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        repo_uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await;
}

/// Rebuilds `repo_uid`'s file list, and appends whatever commits are new
/// since [`crate::storage::search_index::SearchIndex::last_indexed_sha`] (or, on a
/// first-ever build, up to [`crate::storage::search_index::MAX_INDEXED_COMMITS`]) —
/// see that module's own doc comment for why this exists at all
/// (`webui::search` reads the result instead of shelling out to `git` on
/// every query) and why this runs detached after a push, same call site
/// and same "no client is waiting on this" reasoning as
/// [`update_primary_language`]. A no-op if `git_dir` is empty
/// (`resolve_head` returns `None`) or already indexed at the current HEAD.
pub async fn update_search_index(ctx: &AppContext, repo_uid: &Uid, git_dir: &std::path::Path) {
    let _guard = ctx.locks.lock(repo_uid.as_str()).await;
    // `resolve_head` only gives the symbolic ref name (`refs/heads/main`),
    // never a commit sha — needed here so `last_indexed_sha` actually
    // changes across pushes instead of comparing the same ref-name string
    // to itself every time, which would look like "nothing changed" even
    // when new commits landed.
    let Ok(Some(head_ref)) = crate::git::browse::resolve_head(git_dir).await else {
        return;
    };
    let Ok(Some(head)) = crate::git::browse::resolve_commit(git_dir, &head_ref).await else {
        return;
    };
    let (existing, etag) = storage::search_index::get_search_index(ctx.backend().as_ref(), &ctx.cipher, repo_uid)
        .await
        .unwrap_or((None, None));
    let existing = existing.unwrap_or_default();
    if existing.last_indexed_sha.as_deref() == Some(head.as_str()) {
        return;
    }
    let Ok(files) = crate::git::browse::search_filenames(git_dir, &head, "").await else {
        return;
    };
    let range = match &existing.last_indexed_sha {
        Some(prev) => format!("{prev}..{head}"),
        None => head.clone(),
    };
    let Ok(new_commits) =
        crate::git::browse::log(git_dir, &range, storage::search_index::MAX_INDEXED_COMMITS, None).await
    else {
        return;
    };
    let mut commits: Vec<storage::search_index::IndexedCommit> = new_commits
        .into_iter()
        .map(|c| storage::search_index::IndexedCommit {
            sha: c.sha,
            subject: c.subject,
        })
        .collect();
    commits.extend(existing.commits);
    commits.truncate(storage::search_index::MAX_INDEXED_COMMITS);

    let index = storage::search_index::SearchIndex {
        last_indexed_sha: Some(head),
        files,
        commits,
    };
    let _ =
        storage::search_index::put_search_index(ctx.backend().as_ref(), &ctx.cipher, repo_uid, &index, etag.as_deref())
            .await;
}

/// Toggles `username`'s watch on `repo_uid` — exact mirror of
/// [`toggle_star`], down to the same `None`-if-user-vanished and
/// leak-guard-stays-in-the-caller reasoning. Watching a repo doesn't
/// currently make anything happen beyond this toggle and the visible
/// count: no notification delivery exists in this codebase yet.
pub async fn toggle_watch(ctx: &AppContext, username: &str, repo_uid: &Uid) -> Result<Option<(Cas, bool)>> {
    // Bounded retry -- see `toggle_star`'s own doc comment on why this one
    // isn't left best-effort like the count it drives.
    const MAX_ATTEMPTS: u8 = 5;
    for _ in 0..MAX_ATTEMPTS {
        let (Some(mut user), etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await? else {
            return Ok(None);
        };
        let now_watched = if user.watched.remove(repo_uid) {
            false
        } else {
            user.watched.insert(repo_uid.clone());
            true
        };
        let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
        if cas == Cas::Ok {
            bump_watcher_count(ctx, repo_uid, now_watched).await;
            return Ok(Some((Cas::Ok, now_watched)));
        }
    }
    Ok(Some((Cas::Conflict, false)))
}

/// Best-effort ±1 on the repo's own `watcher_count` — exact mirror of
/// [`bump_star_count`].
async fn bump_watcher_count(ctx: &AppContext, repo_uid: &Uid, now_watched: bool) {
    let Ok((mut config, etag)) =
        storage::settings::get_repo_config(ctx.backend().as_ref(), repo_uid.as_str(), &ctx.cipher).await
    else {
        return;
    };
    if now_watched {
        config.watcher_count = config.watcher_count.saturating_add(1);
    } else {
        config.watcher_count = config.watcher_count.saturating_sub(1);
    }
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        repo_uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await;
}
