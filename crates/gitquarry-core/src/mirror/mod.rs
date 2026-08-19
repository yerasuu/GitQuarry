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

pub use hook::run_pre_receive_hook;

use crate::app::{AppContext, Mirror};
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

/// Downloads missing packs and makes the local mirror's refs/HEAD match the
/// S3 manifest exactly (creates, moves, and deletes as needed).
pub async fn sync_bare_repo_from_s3(remote: &crate::storage::remote::Remote, bare_repo: &Path) -> Result<()> {
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

    let (manifest, _etag) = remote.get_manifest().await?;
    let local_refs = git::list_refs(bare_repo).await?;

    for (name, sha) in &manifest.refs {
        git::update_ref(bare_repo, name, sha).await?;
    }
    for name in &local_refs {
        if !manifest.refs.contains_key(name) {
            git::delete_ref(bare_repo, name).await?;
        }
    }
    if let Some(head) = &manifest.head
        && manifest.refs.contains_key(head)
    {
        git::set_head(bare_repo, head).await?;
    }

    Ok(())
}

/// Syncs a peer-repo mirror via a real `git fetch` against the peer's own
/// URL, instead of S3 — federation layer 2 (`docs/federation-concept.md`),
/// [`sync_bare_repo_from_s3`]'s counterpart for
/// [`crate::app::AppContext::open_remote_mirror`]. Thin wrapper: all the
/// actual git plumbing (`--prune`, HEAD-symref discovery) lives in
/// [`git::fetch_remote_url`].
pub async fn sync_bare_repo_from_remote_url(bare_repo: &Path, url: &str) -> Result<()> {
    git::fetch_remote_url(bare_repo, url).await
}

/// How a local bare mirror gets its refs/packs made current — the one seam
/// between `AppContext::open_mirror`/`open_remote_mirror_at` (both otherwise
/// identical: validate path, lock, [`ensure_bare_repo`], sync, `gc::touch_last_access`)
/// and which of the two real sync mechanisms this codebase already has
/// applies. Not `#[async_trait]`/`dyn`-boxed: both call sites know their
/// concrete strategy at compile time, so a plain generic parameter is
/// enough — no need for the extra indirection [`crate::mirror::external::forge::ForgeApi`]'s
/// dynamic dispatch earns itself (there, the provider is only known at
/// runtime from `ExternalMirror.provider`).
pub(crate) trait SyncStrategy {
    async fn sync(&self, bare_repo: &Path) -> Result<()>;
}

/// [`AppContext::open_mirror`]'s strategy: this bucket's own S3 manifest.
pub(crate) struct FromBucket<'a> {
    pub remote: &'a crate::storage::remote::Remote,
}

impl SyncStrategy for FromBucket<'_> {
    async fn sync(&self, bare_repo: &Path) -> Result<()> {
        sync_bare_repo_from_s3(self.remote, bare_repo).await
    }
}

/// [`AppContext::open_remote_mirror_at`]'s strategy: a real `git fetch`
/// against a linked peer's own URL.
pub(crate) struct FromRemoteUrl<'a> {
    pub url: &'a str,
}

impl SyncStrategy for FromRemoteUrl<'_> {
    async fn sync(&self, bare_repo: &Path) -> Result<()> {
        sync_bare_repo_from_remote_url(bare_repo, self.url).await
    }
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
