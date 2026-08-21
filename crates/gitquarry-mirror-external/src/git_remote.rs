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
use std::time::Duration;
use tokio::process::Command;

/// Hard bound on one `git` call against an external forge. Nothing here is
/// interactive, and every one of these runs while the repo's own lock is
/// held (`AppContext::open_mirror`), so a forge that accepts the connection
/// and then stalls forever would otherwise pin that lock — and with it
/// every clone/push of the repo — for as long as the process lives.
const GIT_OP_TIMEOUT: Duration = Duration::from_secs(300);

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
    let mut full: Vec<&OsStr> = credential_args().to_vec();
    full.extend_from_slice(args);
    let mut cmd = Command::new("git");
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
    let mut args: Vec<&OsStr> = vec![OsStr::new("push"), OsStr::new("--end-of-options"), OsStr::new(url)];
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
