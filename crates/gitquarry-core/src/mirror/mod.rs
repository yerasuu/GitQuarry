//! Local bare mirror lifecycle: create it, install its `pre-receive` hook,
//! sync it against S3, and evict it once it's cold (see [`gc`]). Shared by
//! [`crate::app::AppContext::open_mirror`] (every transport goes through it
//! before touching a mirror) and every network server directly —
//! [`crate::git::ssh`] and [`crate::git::http`] launch `git upload-pack`/
//! `receive-pack` via [`spawn_git_subcommand`] the same way. [`hook`] (the
//! `pre-receive` hook this module's mirrors run through) is a separate
//! concern re-exported here for callers that reach it as
//! `mirror::run_pre_receive_hook`.

pub mod deleted_repo_sweep;
pub mod gc;
#[cfg(any(test, feature = "test-util"))]
pub mod hook;
#[cfg(not(any(test, feature = "test-util")))]
mod hook;
pub mod policy;
pub mod recent;

pub use hook::run_pre_receive_hook;

use crate::app::{AppContext, Mirror, ResolvedRepo};
use crate::git;
use anyhow::{Context, Result};
use futures::stream::{self, TryStreamExt};
use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, Command};

/// How many packs [`sync_bare_repo_from_s3`] downloads at once. Bounded, not
/// all-at-once: a repo with hundreds of small packs (pre-`repack`) shouldn't
/// open hundreds of simultaneous connections to the storage backend.
const DOWNLOAD_CONCURRENCY: usize = 8;

/// The hook point `gitquarry-mirror-external`'s sync engine plugs into —
/// core can't depend on that crate directly (it depends on core, for
/// `AppContext`/`storage`/`git`; the reverse would cycle), so this trait is
/// the seam: core defines *where* a sync needs to happen and *when*,
/// `gitquarry-server` (which already depends on both) wires the real
/// implementation into [`AppContext::mirror_sync`] at startup. `None` (no
/// binary/deploy wired it up, or the feature is compiled out) just means
/// nothing runs here — same "`None` = feature fully off" pattern
/// `AppContext.tracker` already establishes.
///
/// `#[async_trait]`/`dyn`-boxed rather than a generic: core has no concrete
/// type to name here at all — whether this is wired, and to what, is a
/// `gitquarry-server`-startup decision core never sees.
#[async_trait::async_trait]
pub trait MirrorSyncHook: Send + Sync {
    /// Called right before serving a clone/fetch (`upload-pack`) — see
    /// [`sync_external_before_serving`], including why a push is
    /// deliberately not one of the triggers.
    /// Implementations must never propagate an error or block
    /// indefinitely: a slow/unreachable external forge must never turn
    /// into a hung or failed git operation, the single most
    /// latency-sensitive path in core (`docs/mirror-external-extraction-concept.md`'s
    /// "never blocks basic git functions" invariant — enforcing the actual
    /// bound, e.g. a timeout, is this implementation's job now, not core's).
    async fn sync_before_serving(&self, ctx: &AppContext, resolved: &ResolvedRepo);
}

/// Lets `#[derive(Debug)]` on [`AppContext`] cover its
/// `Option<Arc<dyn MirrorSyncHook>>` field without the trait itself needing
/// a `Debug` supertrait every implementation would have to satisfy.
impl std::fmt::Debug for dyn MirrorSyncHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<mirror sync hook>")
    }
}

/// Calls [`AppContext::mirror_sync`]'s hook, if any is wired, right before
/// serving a clone/fetch (`upload-pack`) — both [`crate::git::ssh`] and
/// [`crate::git::http`] call this immediately before their own
/// [`AppContext::open_mirror`] call, so a client sees the freshest state the
/// wired-in sync can produce.
///
/// Reads only: a push is deliberately *not* a trigger. Pulling from the
/// external side and then accepting the push means the pull may have moved
/// the branch first, leaving the pre-receive hook to reject the client's
/// push as stale — a sync nobody asked for breaking a valid push. Whatever
/// the push lands goes out on the next sweep regardless.
///
/// Skips entirely (zero overhead, not even the virtual call) when either no
/// hook is wired at all, or this repo has no enabled external mirror —
/// checked via the same indexed hint the periodic sweep's own repo
/// discovery uses, so the overwhelming majority of repos (no external
/// mirror configured) pay nothing beyond one indexed lookup.
pub async fn sync_external_before_serving(ctx: &AppContext, resolved: &ResolvedRepo) {
    let Some(hook) = &ctx.mirror_sync else {
        return;
    };
    if !crate::storage::settings::has_enabled_mirror(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid).await {
        return;
    }
    hook.sync_before_serving(ctx, resolved).await;
}

/// Creates the local bare mirror if missing and installs the `pre-receive`
/// hook that syncs every push against S3 (with `exe_path` pointed to by
/// absolute path, so the hook doesn't depend on the binary being in PATH).
pub async fn ensure_bare_repo(bare_repo: &Path, exe_path: &Path) -> Result<()> {
    if !bare_repo.join("HEAD").is_file() {
        git::init_bare(bare_repo).await?;
    }

    let hooks_dir = bare_repo.join("hooks");
    tokio::fs::create_dir_all(&hooks_dir)
        .await
        .with_context(|| format!("creating {}", hooks_dir.display()))?;

    let hook_path = hooks_dir.join("pre-receive");
    let script = format!(
        "#!/bin/sh\nexec {} pre-receive-hook\n",
        shell_quote(&exe_path.to_string_lossy())
    );
    tokio::fs::write(&hook_path, script)
        .await
        .with_context(|| format!("writing {}", hook_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 0700: the hook runs our binary, we don't want another user on the
        // system to be able to rewrite it.
        tokio::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o700))
            .await
            .with_context(|| format!("setting permissions on {}", hook_path.display()))?;
    }

    Ok(())
}

/// POSIX sh-style single-quoting, to put an arbitrary path into the hook
/// script without a `'` or a space breaking (or injecting into) the command.
#[cfg(any(test, feature = "test-util"))]
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
#[cfg(not(any(test, feature = "test-util")))]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Name of the marker file inside a mirror recording the manifest etag it
/// was last fully synced to — see [`sync_bare_repo_from_s3`]'s fast path.
const SYNC_MARKER: &str = ".gitquarry-synced-etag";

/// Downloads missing packs and makes the local mirror's refs/HEAD match the
/// S3 manifest exactly (creates, moves, and deletes as needed).
///
/// This runs on every `info/refs`, `upload-pack` *and* every webui page
/// view of a repo (`AppContext::open_mirror`), not just a push, so for a
/// repo nobody has pushed to since the last time it was opened, the fast
/// path below matters more than the batching in the rest of this function:
/// it skips the S3 `list_pack_names` call (never cached by
/// `s3db::CachedBackend`, which only caches point `get`s) and the ref diff
/// entirely, leaving only the manifest `get` -- itself usually a memory hit
/// off that same cache -- between a browse/clone and local-disk speed.
/// `true` when `bare_repo` already matches the manifest the bucket holds —
/// exactly the condition [`sync_bare_repo_from_s3`]'s fast path checks, but
/// asked *before* taking a write lock, so a reader that needs no sync never
/// takes one at all (`app::AppContext::open_mirror_read`).
///
/// `false` for a mirror that doesn't exist yet, for a bucket whose driver
/// gives no etag, and for any read error on the marker: all of them mean
/// "can't prove it's current", and the only cost of being wrong that way is
/// taking the exclusive path that was the sole path before.
pub async fn is_synced(remote: &crate::storage::remote::Remote, bare_repo: &Path) -> Result<bool> {
    let (_manifest, etag) = remote.get_manifest().await?;
    let Some(etag) = etag else {
        return Ok(false);
    };
    let marker = tokio::fs::read_to_string(bare_repo.join(SYNC_MARKER)).await;
    Ok(marker.is_ok_and(|m| m.trim() == etag))
}

pub async fn sync_bare_repo_from_s3(remote: &crate::storage::remote::Remote, bare_repo: &Path) -> Result<()> {
    let (manifest, etag) = remote.get_manifest().await?;

    // Packs are always uploaded before the manifest CAS that references
    // them (`mirror::hook`'s push path uploads, then calls `put_manifest`),
    // so an unchanged etag guarantees the packs and refs below are already
    // exactly what this mirror has. The marker lives as a file *inside*
    // `bare_repo`, not an in-process cache keyed by uid: `mirror::gc`'s
    // eviction sweep deletes this whole directory when it evicts a cold
    // mirror, and it can't do that without taking this file down with it --
    // an in-process cache disconnected from the mirror's real on-disk state
    // could otherwise skip a sync into a mirror eviction had just emptied.
    let marker_path = bare_repo.join(SYNC_MARKER);
    if let Some(etag) = etag.as_deref()
        && let Ok(marker) = tokio::fs::read_to_string(&marker_path).await
        && marker.trim() == etag
    {
        return Ok(());
    }

    let remote_packs = remote.list_pack_names().await?;
    let local_packs = git::local_pack_names(bare_repo).await?;
    let dest_dir = git::objects_pack_dir(bare_repo);
    tokio::fs::create_dir_all(&dest_dir)
        .await
        .with_context(|| format!("creating {}", dest_dir.display()))?;
    let missing: Vec<String> = remote_packs.into_iter().filter(|b| !local_packs.contains(b)).collect();
    let dest_dir: &Path = &dest_dir;
    stream::iter(missing.iter().map(Ok::<_, anyhow::Error>))
        .try_for_each_concurrent(Some(DOWNLOAD_CONCURRENCY), |base_name| async move {
            remote.download_pack(base_name, dest_dir).await
        })
        .await?;

    // Diffed against the manifest before writing anything, and applied in
    // one batched `git update-ref --stdin` call: this sync runs on every
    // `info/refs` and `upload-pack` (not just a push), so without the diff
    // every one of those would rewrite every ref via its own subprocess
    // even when nothing had actually changed since the last sync.
    let local_refs = git::list_refs_with_oids(bare_repo).await?;
    let updates: Vec<(String, String)> = manifest
        .refs
        .iter()
        .filter(|(name, sha)| local_refs.get(*name).map(String::as_str) != Some(sha.as_str()))
        .map(|(name, sha)| (name.clone(), sha.clone()))
        .collect();
    let deletes: Vec<String> = local_refs
        .keys()
        .filter(|name| !manifest.refs.contains_key(*name))
        .cloned()
        .collect();
    git::apply_ref_changes(bare_repo, &updates, &deletes).await?;
    if let Some(head) = &manifest.head
        && manifest.refs.contains_key(head)
    {
        git::set_head(bare_repo, head).await?;
    }

    // Best-effort: a failure to write the marker just means the next call
    // takes the slow path again instead of wrongly skipping a real sync.
    if let Some(etag) = &etag {
        let _ = tokio::fs::write(&marker_path, etag).await;
    }

    Ok(())
}

/// A repo's display name and uid, always passed down to a spawned git
/// subprocess together (`GITQUARRY_REPO`/`GITQUARRY_REPO_UID`) — bundled so
/// [`spawn_git_subcommand`]/[`spawn_git_subcommand_http`] don't each carry
/// two more positional string-ish params on top of everything else they
/// already take.
pub(crate) struct RepoRef<'a> {
    pub name: &'a str,
    pub uid: &'a crate::storage::uid::Uid,
}

/// Builds and starts `git upload-pack`/`git receive-pack` against an
/// already-open mirror, with the same env vars the `pre-receive` hook needs
/// to find the bucket (`GITQUARRY_BUCKET`, `GITQUARRY_REPO_UID`,
/// `GITQUARRY_CONFIG`). Used by the SSH server (`crate::git::ssh`, over an
/// SSH channel) — only where stdin/stdout gets pumped from/to changes
/// across callers, not how the process is launched.
///
/// `repo.name` is passed too (`GITQUARRY_REPO`) but only for the hook's own
/// log/error text — the hook resolves its actual S3 prefix from
/// `repo.uid` directly, never by re-resolving `repo.name` through the
/// repo-name index itself: that resolution already happened once in this
/// same request (see `AppContext::resolve_repo`/`create_repo`), and a hook
/// subprocess re-doing it independently would just add a second, racy
/// lookup instead of trusting the answer the parent process already has.
///
/// `actor`: the authenticated username, if any — `None` for an anonymous
/// caller (e.g. an unauthenticated read of a `Public` repo). Passed through
/// as `GITQUARRY_ACTOR`, which the pre-receive hook reads to decide who may
/// push directly to a protected branch; git itself hands its own env down
/// to hooks unchanged, so setting it here is enough for the hook process to
/// see it later.
pub(crate) fn spawn_git_subcommand(
    ctx: &AppContext,
    git_subcmd: &str,
    repo: RepoRef<'_>,
    mirror: &Mirror,
    protocol_extra: Option<&str>,
    stderr: Stdio,
    actor: Option<&str>,
) -> Result<Child> {
    // No `--end-of-options`: upload-pack/receive-pack don't accept it. Not
    // needed anyway, `mirror.path` is absolute and we build it ourselves
    // from an already-sanitized name, it never starts with '-'.
    let mut cmd = Command::new("git");
    cmd.arg(git_subcmd)
        .arg(&mirror.path)
        .env("GIT_DIR", &mirror.path)
        .env("GITQUARRY_BUCKET", &ctx.bucket)
        .env("GITQUARRY_REPO", repo.name)
        .env("GITQUARRY_REPO_UID", repo.uid.as_str())
        .env("GITQUARRY_CONFIG", &ctx.config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr);
    if let Some(extra) = protocol_extra {
        cmd.env("GIT_PROTOCOL", extra);
    }
    if let Some(actor) = actor {
        cmd.env("GITQUARRY_ACTOR", actor);
    }
    cmd.spawn().with_context(|| format!("couldn't run git {git_subcmd}"))
}

/// Same env-var wiring as [`spawn_git_subcommand`], but for git's
/// stateless-rpc mode: one request/response per call instead of a
/// persistent bidirectional channel, the shape git smart-HTTP needs (see
/// `crate::git::http`). `advertise_refs`: `--advertise-refs`, for the
/// initial `GET .../info/refs` advertisement; `false` for the actual
/// `POST .../git-upload-pack`/`git-receive-pack` negotiation. stdin/stdout/
/// stderr are all piped: the caller writes the request body (if any) to
/// stdin, then reads stdout/stderr to completion — see
/// `crate::git::http::run_stateless_rpc`.
pub(crate) fn spawn_git_subcommand_http(
    ctx: &AppContext,
    git_subcmd: &str,
    repo: RepoRef<'_>,
    mirror: &Mirror,
    advertise_refs: bool,
    actor: Option<&str>,
) -> Result<Child> {
    let mut cmd = Command::new("git");
    cmd.arg(git_subcmd).arg("--stateless-rpc");
    if advertise_refs {
        cmd.arg("--advertise-refs");
    }
    // No `--end-of-options`: upload-pack/receive-pack don't accept it (see
    // `spawn_git_subcommand`'s same note).
    cmd.arg(&mirror.path)
        .env("GIT_DIR", &mirror.path)
        .env("GITQUARRY_BUCKET", &ctx.bucket)
        .env("GITQUARRY_REPO", repo.name)
        .env("GITQUARRY_REPO_UID", repo.uid.as_str())
        .env("GITQUARRY_CONFIG", &ctx.config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(actor) = actor {
        cmd.env("GITQUARRY_ACTOR", actor);
    }
    cmd.spawn()
        .with_context(|| format!("couldn't run git {git_subcmd} --stateless-rpc"))
}
