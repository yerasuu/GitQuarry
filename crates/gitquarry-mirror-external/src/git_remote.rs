//! git plumbing exclusive to external-mirror sync (push/fetch/ls-remote
//! against a non-gitquarry forge, plus URL redaction) — moved verbatim out
//! of `gitquarry-core::git` (see `docs/mirror-external-extraction-concept.md`
//! Phase 1's audit: zero callers anywhere else in the workspace). Shared
//! kernel git primitives this engine still needs (`is_ancestor`,
//! `scratch_dir`, `pack_objects_for_push`, `update_ref`) stay in
//! `gitquarry_core::git` instead — this module only owns its own tiny
//! `run`/`run_text` wrapper because those four don't need it duplicated.

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::process::Command;

/// Hard bound on one `git` call against an external forge. Nothing here is
/// interactive, and every one of these runs while the repo's own lock is
/// held (`AppContext::open_mirror`), so a forge that accepts the connection
/// and then stalls forever would otherwise pin that lock — and with it
/// every clone/push of the repo — for as long as the process lives.
const GIT_OP_TIMEOUT: Duration = Duration::from_secs(300);

/// Strips `cmd` to what a git touching untrusted objects may see (§4): none
/// of this process's environment but `PATH` (its other credentials, a `GIT_*`
/// variable, proxy or askpass settings), and no user or system git config, so
/// only the flags and `GIT_DIR` the caller sets next configure it. Call it
/// before setting anything else on `cmd`, since it clears the environment.
///
/// `GIT_CONFIG_NOSYSTEM`/`GIT_CONFIG_GLOBAL`:
/// [git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-GITCONFIGNOSYSTEM).
/// HOME is still set, to a directory with nothing in it, because git expands
/// `~` and looks for XDG config under it.
///
/// # Errors
/// When the empty HOME can't be created.
pub(crate) fn isolate(cmd: &mut std::process::Command) -> Result<()> {
    static EMPTY_HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    let home = match EMPTY_HOME.get() {
        Some(home) => home,
        None => {
            let made = gitquarry_core::git::scratch_dir(&std::env::temp_dir())?;
            EMPTY_HOME.get_or_init(|| made)
        }
    };
    cmd.env_clear()
        .env("HOME", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    Ok(())
}

async fn run(git_dir: Option<&Path>, args: &[&OsStr]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    if let Some(dir) = git_dir {
        cmd.env("GIT_DIR", dir);
    }
    cmd.args(args);
    // Without this a dropped future (the pre-serve sync's own timeout, a
    // cancelled request) leaves the child running detached — still writing
    // into the bare repo after the repo lock the caller held is already
    // gone, racing whoever takes it next.
    cmd.kill_on_drop(true);
    let out = tokio::time::timeout(GIT_OP_TIMEOUT, cmd.output())
        .await
        .map_err(|_elapsed| anyhow::anyhow!("git {} timed out after {GIT_OP_TIMEOUT:?}", describe(args)))?
        .with_context(|| format!("couldn't run git {}", describe(args)))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            describe(args),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Env var the credential helper below reads the token out of. The token
/// must never reach `git`'s argv: `/proc/<pid>/cmdline` is world-readable,
/// so every local account on the box could read a mirror's forge token for
/// as long as a sync ran. An environment is readable only by the same uid
/// (and root), which is the actual improvement here — the helper *script*
/// still shows up in argv, but it carries no secret.
const TOKEN_ENV: &str = "GITQUARRY_MIRROR_TOKEN";

/// `git` flags that make it answer its own credential prompt from
/// [`TOKEN_ENV`] instead of asking anybody. The empty `credential.helper=`
/// first wipes whatever the system/global config might have configured, so
/// only this one runs.
fn credential_args() -> [&'static OsStr; 8] {
    [
        OsStr::new("-c"),
        OsStr::new("credential.helper="),
        OsStr::new("-c"),
        OsStr::new(
            "credential.helper=!f() { echo username=x-access-token; echo \"password=$GITQUARRY_MIRROR_TOKEN\"; }; f",
        ),
        OsStr::new("-c"),
        OsStr::new("credential.interactive=false"),
        // The host allowlist decides where a mirror may point this server;
        // a redirect would let an allowed host hand that decision to
        // somewhere else after the check passed. git's own default follows
        // the initial request, which is exactly the case that matters here.
        OsStr::new("-c"),
        OsStr::new("http.followRedirects=false"),
    ]
}

/// [`run`] with a forge token attached out of band — see [`TOKEN_ENV`].
/// `url` stays the plain `https://host/owner/repo` the mirror stored, so
/// neither argv nor an error string can leak the credential.
async fn run_authed(git_dir: Option<&Path>, token: &str, args: &[&OsStr]) -> Result<Vec<u8>> {
    run_authed_with(Command::new("git"), git_dir, token, args).await
}

async fn run_authed_with(mut cmd: Command, git_dir: Option<&Path>, token: &str, args: &[&OsStr]) -> Result<Vec<u8>> {
    let mut full: Vec<&OsStr> = credential_args().to_vec();
    full.extend_from_slice(args);
    if let Some(dir) = git_dir {
        cmd.env("GIT_DIR", dir);
    }
    cmd.env(TOKEN_ENV, token);
    // Belt and braces with `credential.interactive=false`: without this a
    // forge that rejects the token can leave git waiting on a terminal
    // that isn't there, burning the whole `GIT_OP_TIMEOUT` every tick.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.args(&full);
    cmd.kill_on_drop(true);
    let out = tokio::time::timeout(GIT_OP_TIMEOUT, cmd.output())
        .await
        .map_err(|_elapsed| anyhow::anyhow!("git {} timed out after {GIT_OP_TIMEOUT:?}", describe(args)))?
        .with_context(|| format!("couldn't run git {}", describe(args)))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            describe(args),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

async fn run_text(git_dir: Option<&Path>, args: &[&OsStr]) -> Result<String> {
    let out = run(git_dir, args).await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

fn describe(args: &[&OsStr]) -> String {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Full ref names with their current sha — [`ls_remote_refs`]'s local-side
/// counterpart, needed by the three-way sync comparison against a mirror's
/// `last_synced` and the external side's own current state.
pub async fn list_refs_with_sha(git_dir: &Path) -> Result<std::collections::HashMap<String, String>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("for-each-ref"),
            OsStr::new("--format=%(refname)%09%(objectname)"),
            OsStr::new("refs/"),
        ],
    )
    .await?;
    let mut refs = std::collections::HashMap::new();
    for line in out.lines() {
        if let Some((name, sha)) = line.split_once('\t') {
            refs.insert(name.to_string(), sha.to_string());
        }
    }
    Ok(refs)
}

/// Every ref `url` currently advertises, as a ref-name -> sha map.
///
/// `--refs`, not a plain `ls-remote`: the default output also carries the
/// peeled `refs/tags/<x>^{}` line for every annotated tag (and `HEAD`),
/// and `^{}` is not a ref name a refspec can name at all — a caller that
/// took those for real refs tried to `fetch`/`push` them and got
/// `fatal: invalid refspec 'refs/tags/<x>^{}'` on every tick, for every
/// repo carrying a single annotated tag.
///
/// `token` reaches `git` through [`TOKEN_ENV`], never through `url` or
/// argv. `Err` still goes through [`redact_url`]: a remote can echo a URL
/// of its own choosing back at us in an error line.
pub async fn ls_remote_refs(url: &str, token: &str) -> Result<std::collections::HashMap<String, String>> {
    let raw = run_authed(
        None,
        token,
        &[
            OsStr::new("ls-remote"),
            OsStr::new("--refs"),
            OsStr::new("--end-of-options"),
            OsStr::new(url),
        ],
    )
    .await
    .map_err(|e| anyhow::anyhow!(redact_url(&e.to_string())))?;
    let out = String::from_utf8_lossy(&raw).trim().to_string();
    let mut refs = std::collections::HashMap::new();
    for line in out.lines() {
        if let Some((sha, name)) = line.split_once('\t') {
            refs.insert(name.to_string(), sha.to_string());
        }
    }
    Ok(refs)
}

/// `git push` against `url`, which does its own wire negotiation — no pack
/// building here, unlike the pre-receive hook's push-to-S3 path.
///
/// Same out-of-band credential as [`ls_remote_refs`].
pub async fn push_to_remote_url(bare_repo: &Path, url: &str, token: &str, refspecs: &[String]) -> Result<()> {
    push(bare_repo, url, token, &[], refspecs).await
}

/// [`push_to_remote_url`] with `--atomic`: every ref moves or none does. A
/// remote that can't do atomic pushes refuses the whole push rather than
/// applying part of it.
pub async fn push_atomic_to_remote_url(bare_repo: &Path, url: &str, token: &str, refspecs: &[String]) -> Result<()> {
    push(bare_repo, url, token, &[OsStr::new("--atomic")], refspecs).await
}

/// [`push_atomic_to_remote_url`] that may also replace the refs named in
/// `leases`, each `(ref, expected)`: git moves such a ref, fast-forward or
/// not, only if the remote still holds `expected` there when the push
/// lands, and an empty `expected` means "must not exist". Never a bare
/// `--force`: a ref somebody moved since `expected` was read refuses the
/// whole atomic push. See
/// [git-push: --force-with-lease](https://git-scm.com/docs/git-push#Documentation/git-push.txt---force-with-leaseltrefnamegtltexpectgt).
pub async fn push_atomic_with_leases(
    bare_repo: &Path,
    url: &str,
    token: &str,
    leases: &[(String, String)],
    refspecs: &[String],
) -> Result<()> {
    let flags: Vec<String> = leases
        .iter()
        .map(|(name, expected)| format!("--force-with-lease={name}:{expected}"))
        .collect();
    let mut all = vec![OsStr::new("--atomic")];
    all.extend(flags.iter().map(OsStr::new));
    push(bare_repo, url, token, &all, refspecs).await
}

async fn push(bare_repo: &Path, url: &str, token: &str, flags: &[&OsStr], refspecs: &[String]) -> Result<()> {
    let mut args: Vec<&OsStr> = vec![OsStr::new("push")];
    args.extend_from_slice(flags);
    args.extend([OsStr::new("--end-of-options"), OsStr::new(url)]);
    args.extend(refspecs.iter().map(OsStr::new));
    run_authed(Some(bare_repo), token, &args)
        .await
        .map_err(|e| anyhow::anyhow!(redact_url(&e.to_string())))?;
    Ok(())
}

/// Fetches the objects for `ref_name` from `url` into `bare_repo` without
/// moving any local ref — the caller already knows the target sha from its
/// own prior [`ls_remote_refs`] call, so it only needs the objects here,
/// then calls `gitquarry_core::git::update_ref` itself once this returns.
/// No `+<ref>:<local-ref>` destination refspec: writing straight to a local
/// ref from a fetch would make git enforce fast-forward-only (or need `+`
/// to force it), and the caller has already decided this is safe (fast-
/// forward or a brand new ref) before ever calling this — a plain
/// source-only refspec just populates objects, `update_ref` after is an
/// explicit, separate step.
///
/// Same out-of-band credential as [`push_to_remote_url`].
pub async fn fetch_ref_from_remote_url(bare_repo: &Path, url: &str, token: &str, ref_name: &str) -> Result<()> {
    run_authed(
        Some(bare_repo),
        token,
        &[
            OsStr::new("fetch"),
            OsStr::new("--end-of-options"),
            OsStr::new(url),
            OsStr::new(ref_name),
        ],
    )
    .await
    .map_err(|e| anyhow::anyhow!(redact_url(&e.to_string())))?;
    Ok(())
}

/// How often [`fetch_untrusted`] measures the quarantine while git writes
/// into it: what git can write between two looks is all it can overshoot by.
const QUARANTINE_POLL: Duration = Duration::from_millis(50);

/// What [`fetch_untrusted`] did with the size budget it was given.
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum Fetched {
    Done,
    /// The quarantine went over the budget, during the fetch or by the end
    /// of it. git was killed, and the refs the refspecs name may be missing
    /// or point at objects that never arrived: the quarantine is only fit
    /// to be thrown away.
    OverBudget,
}

/// Fetches `refspecs` from `url` into `quarantine`, a bare repo of its own,
/// for content nobody has vetted yet (a public pull request,
/// `docs/publish-mirror-inbound-concept.md` §3.1 and §3.12): every object is
/// fsck'd as it arrives, no hook can run, and neither tags nor submodules
/// come along. Bounded in time only by [`GIT_OP_TIMEOUT`]; a caller with a
/// tighter budget wraps the future in its own timeout, and dropping it kills
/// git.
///
/// Bounded in disk by `max_bytes`, everything under `quarantine` counted,
/// watched while git runs: git has no client-side cap on what a fetch takes
/// in (`receive.maxInputSize` binds only the receiving end of a push,
/// [git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-receivemaxInputSize)),
/// so a remote could otherwise fill the disk for as long as the timeout
/// lets it.
///
/// git runs with none of this process's environment but `PATH`, and with no
/// user or system config: only the flags set here configure it.
///
/// Same out-of-band credential as [`fetch_ref_from_remote_url`].
pub async fn fetch_untrusted(
    quarantine: &Path,
    url: &str,
    token: &str,
    refspecs: &[String],
    max_bytes: u64,
) -> Result<Fetched> {
    let fetch = fetch_untrusted_unbounded(quarantine, url, token, refspecs);
    let watch = async {
        let mut tick = tokio::time::interval(QUARANTINE_POLL);
        loop {
            tick.tick().await;
            if quarantine_bytes(quarantine).await? > max_bytes {
                return anyhow::Ok(());
            }
        }
    };
    // Losing the race drops the fetch future, and with it the child: kill_on_drop
    // kills `git fetch`, and the `index-pack` it feeds dies on the early EOF.
    tokio::select! {
        fetched = fetch => fetched?,
        over = watch => {
            over?;
            return Ok(Fetched::OverBudget);
        }
    }
    if quarantine_bytes(quarantine).await? > max_bytes {
        return Ok(Fetched::OverBudget);
    }
    Ok(Fetched::Done)
}

async fn quarantine_bytes(quarantine: &Path) -> Result<u64> {
    let dir = quarantine.to_path_buf();
    tokio::task::spawn_blocking(move || bytes_on_disk(&dir))
        .await
        .context("the quarantine size walk panicked")?
}

/// Bytes under `dir`, every file counted by its own size. Symlinks are
/// counted as links and never followed: git writes none in a bare repo,
/// and one would otherwise let the count wander outside it. An entry that
/// vanishes mid-walk is git renaming or dropping a temporary file, not an
/// error.
fn bytes_on_disk(dir: &Path) -> Result<u64> {
    let gone = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
    let mut total = 0u64;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if gone(&error) => continue,
            Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
            let meta = match std::fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(error) if gone(&error) => continue,
                Err(error) => return Err(error).with_context(|| format!("reading {}", entry.path().display())),
            };
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}

async fn fetch_untrusted_unbounded(quarantine: &Path, url: &str, token: &str, refspecs: &[String]) -> Result<()> {
    let mut args: Vec<&OsStr> = [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "fetch.fsckObjects=true",
        "-c",
        "transfer.fsckObjects=true",
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-recurse-submodules",
        "--no-write-fetch-head",
        "--end-of-options",
    ]
    .into_iter()
    .map(OsStr::new)
    .collect();
    args.push(OsStr::new(url));
    args.extend(refspecs.iter().map(OsStr::new));
    // §4, see [`isolate`].
    let mut cmd = Command::new("git");
    isolate(cmd.as_std_mut())?;
    run_authed_with(cmd, Some(quarantine), token, &args)
        .await
        .map_err(|e| anyhow::anyhow!(redact_url(&e.to_string())))?;
    Ok(())
}

/// [`fetch_ref_from_remote_url`] at depth 1: only the tip commit and its
/// tree, enough for a local commit built on top of it to be pushed as a
/// fast-forward without the remote's whole history.
pub async fn fetch_tip_from_remote_url(bare_repo: &Path, url: &str, token: &str, ref_name: &str) -> Result<()> {
    run_authed(
        Some(bare_repo),
        token,
        &[
            OsStr::new("fetch"),
            OsStr::new("--depth=1"),
            OsStr::new("--end-of-options"),
            OsStr::new(url),
            OsStr::new(ref_name),
        ],
    )
    .await
    .map_err(|e| anyhow::anyhow!(redact_url(&e.to_string())))?;
    Ok(())
}

/// Strips the userinfo segment (`://<user>:<token>@` -> `://`) from every
/// URL-shaped substring of `text` — not just the first: a `git` failure
/// message can embed the same credentialed URL twice (once in the argv
/// [`describe`]s, once echoed back in stderr on an auth failure), and both
/// occurrences must go before this ever reaches a log line or the web UI's
/// error text for an `ExternalMirror` sync. Scans for `://` rather than
/// parsing a real URL type: cheap, and every caller here only ever has one
/// scheme (`https://`) to worry about.
pub fn redact_url(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_pos) = rest.find("://") {
        let scheme_end = scheme_pos + 3;
        out.push_str(&rest[..scheme_end]);
        let after_scheme = &rest[scheme_end..];
        let boundary = after_scheme
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after_scheme.len());
        rest = match after_scheme[..boundary].find('@') {
            Some(at_pos) => &after_scheme[at_pos + 1..],
            None => after_scheme,
        };
    }
    out.push_str(rest);
    out
}
