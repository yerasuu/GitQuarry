//! Everything git-protocol-shaped lives under this module — the two public
//! transports ([`ssh`], [`http`]) plus the plumbing they (and the
//! pre-receive hook) share:
//! - this file: thin wrappers over the `git` binary, protocol-level:
//!   pack/ref plumbing.
//! - [`ssh`]: the embedded `ssh://` server.
//! - [`http`]: git smart-HTTP, layered as middleware onto the web server's
//!   router (see its own module doc for why) — the only piece here that
//!   isn't purely transport-agnostic plumbing, since it has to cooperate
//!   with [`crate::webui`]'s router instead of owning its own listener.
//!
//! Read-heavy operations for the web UI (log, tree, blame, archive, ...)
//! live in [`browse`] instead — different concern, different consumers:
//! that's browsing-UI support code, not protocol serving.
//!
//! Two rules that apply to the whole module (and `browse`):
//! - git's output is decoded with `from_utf8_lossy`: file names, authors and
//!   commit messages aren't guaranteed to be utf8, and failing the whole
//!   request over one stray byte would be worse than showing a `U+FFFD`.
//! - any rev/path coming from outside goes after `--end-of-options`, so git
//!   doesn't interpret it as a flag even if it starts with `-`.

pub mod browse;
pub mod codeowners;
pub mod http;
#[cfg(any(test, feature = "test-util"))]
pub mod lfs;
#[cfg(not(any(test, feature = "test-util")))]
mod lfs;
pub mod signing;
pub mod ssh;

use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Runs `git <args>` (with `GIT_DIR` if given) and returns raw stdout.
/// Takes an explicit `git_dir` instead of inheriting the environment because
/// the server handles several repos at once in the same process.
async fn run(git_dir: Option<&Path>, args: &[&OsStr]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    if let Some(dir) = git_dir {
        cmd.env("GIT_DIR", dir);
    }
    cmd.args(args);
    let out = cmd
        .output()
        .await
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

/// Same as [`run`] but refuses to buffer more than `max_bytes` of stdout,
/// killing the child the moment it goes over.
///
/// `run` buffers whatever git decides to emit, which is fine for the
/// bounded output every other command here produces (a ref list, a diff, a
/// blob already size-checked by its caller) and wrong for `git archive`,
/// whose output scales with the whole tree. `webui::export::snapshot`
/// reaches that path anonymously and unthrottled, so without a ceiling one
/// request could size its own memory footprint off the biggest public repo
/// on the instance.
///
/// This bounds *one* request; concurrent requests still add up. Streaming
/// the archive straight to the client would remove that too, but not while
/// zip repacking needs the whole tar in hand — see
/// [`crate::git::browse::archive_one`].
#[cfg(any(test, feature = "test-util"))]
pub async fn run_capped(git_dir: Option<&Path>, args: &[&OsStr], max_bytes: u64) -> Result<Vec<u8>> {
    run_capped_impl(git_dir, args, max_bytes).await
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) async fn run_capped(git_dir: Option<&Path>, args: &[&OsStr], max_bytes: u64) -> Result<Vec<u8>> {
    run_capped_impl(git_dir, args, max_bytes).await
}

async fn run_capped_impl(git_dir: Option<&Path>, args: &[&OsStr], max_bytes: u64) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let mut cmd = Command::new("git");
    if let Some(dir) = git_dir {
        cmd.env("GIT_DIR", dir);
    }
    let mut child = cmd
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("couldn't run git {}", describe(args)))?;
    let mut stdout = child.stdout.take().context("git stdout was not piped")?;

    let cap = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = stdout.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        if out.len() + n > cap {
            // Don't finish reading just to throw it away: stop the producer.
            let _ = child.kill().await;
            bail!("git {} produced more than the {max_bytes}-byte limit", describe(args));
        }
        out.extend_from_slice(&buf[..n]);
    }

    let status = child.wait().await?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr).await;
        }
        bail!("git {} failed: {}", describe(args), stderr.trim());
    }
    Ok(out)
}

/// Same as [`run`] but returns stdout as trimmed text.
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

/// Temp dir for building packs, created INSIDE `git_dir` (not the system
/// /tmp): `git pack-objects` writes its temp file and renames it to the
/// final destination, and that rename fails with "Invalid cross-device
/// link" if the destination is on a different filesystem than
/// `.git/objects` (typical when /tmp is tmpfs and the repo is on a regular
/// disk).
pub fn scratch_dir(git_dir: &Path) -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(".gitquarry-tmp-")
        .tempdir_in(git_dir)
        .with_context(|| format!("creating temp dir in {}", git_dir.display()))
}

/// true if `ancestor` is an ancestor (or equal to) `descendant`, both
/// resolved against `git_dir` — unlike every other object/rev this module
/// looks up, `ancestor`/`descendant` here can come from two different
/// repos' own idea of a ref's sha (`mirror::external::plan_ref_sync`'s
/// local-vs-remote comparison), so both must already be present as real
/// objects in `git_dir` (the caller's job, e.g. via a fetch) before this is
/// called — this only asks whether the connection between them exists, not
/// how to make it exist.
pub async fn is_ancestor(git_dir: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let status = Command::new("git")
        .current_dir(git_dir)
        .args(["merge-base", "--is-ancestor", "--end-of-options"])
        .args([ancestor, descendant])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("couldn't run git merge-base")?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base --is-ancestor exited with unexpected status: {status:?}"),
    }
}

/// Fetches `src_rev` from the local bare repo at `src_git_dir` into
/// `dest_git_dir` under `local_ref` — a cross-fork merge request's target
/// mirror pulling in the fork's objects, without the two repos sharing any
/// storage (see `domain::merge_request`'s own doc comment: forks are fully
/// independent S3 prefixes and bare mirrors, there's no other path for one
/// mirror to see another's objects). `local_ref` is never written back to
/// `dest_git_dir`'s own manifest/CAS state — it's scratch, fine that the
/// next `mirror::sync_bare_repo_from_s3` deletes it; the next `diff`/`merge`
/// just re-fetches (idempotent, already-present objects are skipped).
///
/// `src_git_dir` is always a server-built path (`mirrors_root.join(uid)`),
/// never client-controlled text; `src_rev` is a branch name the caller
/// already ran through `validate::validate_rev` (no `:` in its accepted
/// charset, so the `{src_rev}:{local_ref}` refspec below can't be
/// ambiguous).
pub async fn fetch_local(dest_git_dir: &Path, src_git_dir: &Path, src_rev: &str, local_ref: &str) -> Result<()> {
    let refspec = format!("{src_rev}:{local_ref}");
    run(
        Some(dest_git_dir),
        &[
            OsStr::new("fetch"),
            OsStr::new("--no-tags"),
            OsStr::new("--end-of-options"),
            src_git_dir.as_os_str(),
            OsStr::new(&refspec),
        ],
    )
    .await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum MergeTreeOutcome {
    /// A clean 3-way merge — the resulting tree's sha, ready for
    /// [`commit_tree`].
    Clean(String),
    /// Conflicting paths, deduplicated and sorted. Nothing was written to
    /// disk beyond `git`'s own internal scratch objects — same as a real
    /// `git merge` leaves nothing committed on conflict.
    Conflicted(Vec<String>),
}

/// Real 3-way merge of `ours`/`theirs` without touching a working tree or
/// the index — `git merge-tree --write-tree`, added in git 2.38. Exit code
/// `1` means conflicts, not a failure: unlike [`run`], this doesn't `bail!`
/// on it, since the caller needs the actual conflicted paths back, not just
/// an error string.
///
/// `merge_base` overrides which commit git treats as the common ancestor
/// (`--merge-base=<commit>`, added in git 2.40) instead of letting git find
/// one itself. A real merge of two branches wants `None` (git's own
/// merge-base search is exactly what a real `git merge` would do); replaying
/// one commit onto a new parent — [`domain::merge_request::merge`]'s rebase
/// strategy — wants `Some(that commit's own first parent)`, which is what
/// turns this 3-way merge into cherry-pick semantics (apply just that
/// commit's own diff, not everything back to whatever git would've picked
/// as the merge base on its own).
///
/// Conflict output shape (verified against git 2.55): stdout is the
/// (partial, conflict-marker-containing) tree oid on line 1, then one line
/// per conflicted stage — `<mode> <blob-sha> <stage>\t<path>` (stage 1/2/3 =
/// base/ours/theirs, so one path can appear on 2-3 lines) — up to a blank
/// line, followed by human-readable "Auto-merging"/"CONFLICT" messages this
/// only needs the paths out of, not the prose.
pub async fn merge_tree(
    git_dir: &Path,
    ours: &str,
    theirs: &str,
    merge_base: Option<&str>,
) -> Result<MergeTreeOutcome> {
    let mut args = vec!["merge-tree".to_string(), "--write-tree".to_string()];
    if let Some(base) = merge_base {
        args.push(format!("--merge-base={base}"));
    }
    args.push("--end-of-options".to_string());
    args.push(ours.to_string());
    args.push(theirs.to_string());
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .args(&args)
        .output()
        .await
        .context("couldn't run git merge-tree")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    match out.status.code() {
        Some(0) => {
            let tree = lines.next().unwrap_or("").trim().to_string();
            if tree.is_empty() {
                bail!("git merge-tree returned no tree sha");
            }
            Ok(MergeTreeOutcome::Clean(tree))
        }
        Some(1) => {
            lines.next(); // the (conflict-marker) tree oid — not useful here
            let mut paths = std::collections::BTreeSet::new();
            for line in lines {
                if line.is_empty() {
                    break; // end of the stage-entry block
                }
                if let Some((_, path)) = line.split_once('\t') {
                    paths.insert(path.to_string());
                }
            }
            Ok(MergeTreeOutcome::Conflicted(paths.into_iter().collect()))
        }
        _ => bail!(
            "git merge-tree exited with unexpected status {:?}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

/// Writes a new commit object with `tree` and `parents`, returning its sha —
/// the plumbing a server-side merge builds its merge/squash/rebased commit
/// with, since there's no working tree/index to run a real `git
/// merge`/`git commit` against a bare mirror. `tree`/`parents` are shas
/// this code already computed/read (trusted); `message` reaches `-m` as a
/// single argv value, so a leading `-` (from an MR title) can't be parsed as
/// a flag — verified directly against git 2.55.
///
/// `author`/`committer` are each `(name, email, date)`, set via
/// `GIT_AUTHOR_*`/`GIT_COMMITTER_*` env vars rather than a commit-tree flag
/// (there isn't one) — both always explicit, never left to fall back to
/// this process's own `git config`, which a bare deploy container has no
/// reason to carry. Real `git rebase` keeps each replayed commit's original
/// author/date and only updates the committer to whoever's rebasing; a
/// regular merge or squash commit instead sets both to whoever's merging,
/// same as today.
pub async fn commit_tree(
    git_dir: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
    author: (&str, &str, &str),
    committer: (&str, &str, &str),
    signing_key: Option<&Path>,
) -> Result<String> {
    // `-c` before the subcommand, not repo config: the mirror is a
    // disposable cache (`mirror::gc` evicts it), so anything written into
    // its own config would have to be re-established on every re-clone.
    // These three only ever apply to this one invocation.
    // `-c name=value` is one argument, not two: split across a pair, git
    // reads the path as the subcommand and reports it as an unknown one.
    // `user.signingkey` takes the *path* to the private key for SSH
    // signing; git shells out to `ssh-keygen -Y sign` with it.
    let signing_setting = signing_key.map(|p| {
        let mut arg = std::ffi::OsString::from("user.signingkey=");
        arg.push(p);
        arg
    });
    let mut args: Vec<&OsStr> = Vec::new();
    if let Some(setting) = signing_setting.as_ref() {
        args.push(OsStr::new("-c"));
        args.push(OsStr::new("gpg.format=ssh"));
        args.push(OsStr::new("-c"));
        args.push(setting);
    }
    args.push(OsStr::new("commit-tree"));
    if signing_setting.is_some() {
        args.push(OsStr::new("-S"));
    }
    args.push(OsStr::new(tree));
    for parent in parents {
        args.push(OsStr::new("-p"));
        args.push(OsStr::new(parent));
    }
    args.push(OsStr::new("-m"));
    args.push(OsStr::new(message));
    let mut cmd = Command::new("git");
    cmd.env("GIT_DIR", git_dir)
        .args(&args)
        .env("GIT_AUTHOR_NAME", author.0)
        .env("GIT_AUTHOR_EMAIL", author.1)
        .env("GIT_AUTHOR_DATE", author.2)
        .env("GIT_COMMITTER_NAME", committer.0)
        .env("GIT_COMMITTER_EMAIL", committer.1)
        .env("GIT_COMMITTER_DATE", committer.2);
    let out = cmd.output().await.context("couldn't run git commit-tree")?;
    if !out.status.success() {
        bail!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// One source-branch commit as [`domain::merge_request::merge`]'s rebase
/// strategy needs to replay it onto a new parent: enough to call
/// [`merge_tree`] with `parent` as the explicit merge-base (cherry-pick
/// semantics) and then [`commit_tree`] with the original author preserved.
///
/// `parent` is `None` for a root commit (no parent to diff against) — the
/// rebase strategy bails on that rather than guessing a merge base, same as
/// it bails on an actual merge commit ([`parents`] holding more than one
/// sha): both are edge cases a linear MR branch doesn't hit in practice, and
/// guessing wrong would silently replay the wrong diff.
///
/// [`parents`]: RebaseCommit::parents
pub struct RebaseCommit {
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub message: String,
}

/// Oldest-first shas in `range` (e.g. `"base..head"`) — the replay order a
/// rebase needs, opposite of `git log`'s default newest-first.
pub async fn rev_list_range(git_dir: &Path, range: &str) -> Result<Vec<String>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("rev-list"),
            OsStr::new("--reverse"),
            OsStr::new("--end-of-options"),
            OsStr::new(range),
        ],
    )
    .await?;
    Ok(out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

/// Field separator matching [`browse::log::LOG_SEP`] — a unit separator
/// can't appear in an author name, and `%B` (the raw, possibly-multiline
/// body) is last so a stray newline in it can't be mistaken for a field
/// boundary.
const REBASE_SEP: &str = "\x1f";

pub async fn commit_detail_for_rebase(git_dir: &Path, sha: &str) -> Result<RebaseCommit> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("log"),
            OsStr::new("-1"),
            OsStr::new("--format=%P\x1f%an\x1f%ae\x1f%aI\x1f%B"),
            OsStr::new("--end-of-options"),
            OsStr::new(sha),
        ],
    )
    .await?;
    let mut parts = out.splitn(5, REBASE_SEP);
    let parents = parts
        .next()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let author_name = parts.next().unwrap_or("").to_string();
    let author_email = parts.next().unwrap_or("").to_string();
    let author_date = parts.next().unwrap_or("").to_string();
    let message = parts.next().unwrap_or("").trim().to_string();
    Ok(RebaseCommit {
        parents,
        author_name,
        author_email,
        author_date,
        message,
    })
}

#[derive(Debug)]
pub struct PackFiles {
    pub base_name: String,
    pub pack_path: PathBuf,
    pub idx_path: PathBuf,
}

/// Packs the objects reachable from `new_sha` that aren't reachable from any
/// of `have_tips`. Returns None if there are no new objects to send.
///
/// `git_dir` is explicit rather than relying on an ambient `GIT_DIR` (unlike
/// most reads in this module, which take an `Option<&Path>` because a
/// missing one just means "use the environment"): this function's original
/// caller, the pre-receive hook (`mirror::hook`), only ever runs as a real
/// git hook, which git itself invokes with `GIT_DIR` already set in the
/// process environment — but `domain::merge_request::merge`'s server-side
/// merge is a second caller that has no such ambient `GIT_DIR` (the main
/// process juggles many repos, never sets one process-wide), so both the
/// `rev-list` and `pack-objects` subprocesses below need it passed in
/// directly.
pub async fn pack_objects_for_push(
    git_dir: &Path,
    new_sha: &str,
    have_tips: &[String],
    out_dir: &Path,
) -> Result<Option<PackFiles>> {
    // Tips to exclude go as `^<rev>` instead of `--not <revs...>`: `--not`
    // is an option and git requires options to come before positional
    // arguments, which is exactly what `--end-of-options` forbids. `^abc`
    // doesn't start with `-`, so it coexists with `--end-of-options`.
    let exclusions: Vec<String> = have_tips.iter().map(|tip| format!("^{tip}")).collect();
    let mut rev_list_args: Vec<&OsStr> = vec![
        OsStr::new("rev-list"),
        OsStr::new("--objects"),
        OsStr::new("--end-of-options"),
        OsStr::new(new_sha),
    ];
    rev_list_args.extend(exclusions.iter().map(OsStr::new));
    pack_objects_from_rev_list(git_dir, &rev_list_args, out_dir).await
}

/// New blob objects `new_sha` introduces on top of `have_tips` (same
/// exclusion set [`pack_objects_for_push`] uses), filtered to ones over
/// `max_size` bytes. Used by the pre-receive hook (`mirror::hook`) to
/// reject a push containing a big blob that was never converted to an LFS
/// pointer client-side — rewriting the blob after the fact isn't an option,
/// the client already signed tree/commit hashes over its real content.
/// Only reads object headers (`cat-file --batch-check`), never blob
/// content, so this stays cheap even for a multi-gigabyte blob.
pub async fn oversized_new_blobs(
    git_dir: &Path,
    new_sha: &str,
    have_tips: &[String],
    max_size: u64,
) -> Result<Vec<(String, u64)>> {
    let exclusions: Vec<String> = have_tips.iter().map(|tip| format!("^{tip}")).collect();
    let mut rev_list_args: Vec<&OsStr> = vec![
        OsStr::new("rev-list"),
        OsStr::new("--objects"),
        OsStr::new("--end-of-options"),
        OsStr::new(new_sha),
    ];
    rev_list_args.extend(exclusions.iter().map(OsStr::new));
    let objects = run(Some(git_dir), &rev_list_args).await?;
    if objects.trim_ascii().is_empty() {
        return Ok(Vec::new());
    }

    // `rev-list --objects` lines are `<sha>[ <path>]` — path present for
    // blobs/trees reached through a named tree entry, absent for commits
    // and any object git can't attach a path to. `cat-file --batch-check`
    // wants one bare sha per line, not the path suffix — feeding it the raw
    // `rev-list` output makes every line "missing" (git tries to resolve
    // "<sha> <path>" as a single revision).
    let text = String::from_utf8_lossy(&objects);
    let mut paths_by_sha: HashMap<&str, &str> = HashMap::new();
    let mut shas = String::new();
    for line in text.lines() {
        let mut it = line.splitn(2, ' ');
        let Some(sha) = it.next() else { continue };
        if let Some(path) = it.next().filter(|p| !p.is_empty()) {
            paths_by_sha.insert(sha, path);
        }
        shas.push_str(sha);
        shas.push('\n');
    }

    let mut child = Command::new("git")
        .env("GIT_DIR", git_dir)
        .arg("cat-file")
        .arg("--batch-check=%(objectname) %(objecttype) %(objectsize)")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run git cat-file --batch-check")?;
    let mut stdin = child.stdin.take().context("cat-file --batch-check stdin")?;
    stdin
        .write_all(shas.as_bytes())
        .await
        .context("writing object list to git cat-file --batch-check")?;
    drop(stdin); // batch-check reads until EOF, same reasoning as pack-objects above.

    let out = child
        .wait_with_output()
        .await
        .context("waiting for git cat-file --batch-check")?;
    if !out.status.success() {
        bail!(
            "git cat-file --batch-check failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let mut oversized = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split(' ');
        let (Some(sha), Some(kind), Some(size)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if kind != "blob" {
            continue;
        }
        let Ok(size) = size.parse::<u64>() else { continue };
        if size > max_size {
            let display = paths_by_sha.get(sha).copied().unwrap_or(sha).to_string();
            oversized.push((display, size));
        }
    }
    Ok(oversized)
}

/// New commit shas `new_sha` introduces on top of `have_tips` — same
/// exclusion-set convention [`oversized_new_blobs`] uses, just plain
/// `rev-list` (no `--objects`) since only commits, not every blob/tree,
/// matter for a signed-commit check. Used by the pre-receive hook
/// (`mirror::hook`) to enumerate exactly the commits a push actually
/// introduces to a `require_signed_commits` branch — not the whole
/// branch's history, which would re-check commits a prior, already-accepted
/// push already covered.
pub async fn new_commit_shas(git_dir: &Path, new_sha: &str, have_tips: &[String]) -> Result<Vec<String>> {
    let exclusions: Vec<String> = have_tips.iter().map(|tip| format!("^{tip}")).collect();
    let mut rev_list_args: Vec<&OsStr> = vec![
        OsStr::new("rev-list"),
        OsStr::new("--end-of-options"),
        OsStr::new(new_sha),
    ];
    rev_list_args.extend(exclusions.iter().map(OsStr::new));
    let out = run(Some(git_dir), &rev_list_args).await?;
    Ok(String::from_utf8_lossy(&out).lines().map(str::to_string).collect())
}

async fn pack_objects_from_rev_list(
    git_dir: &Path,
    rev_list_args: &[&OsStr],
    out_dir: &Path,
) -> Result<Option<PackFiles>> {
    let objects = run(Some(git_dir), rev_list_args).await?;
    if objects.trim_ascii().is_empty() {
        return Ok(None);
    }

    let prefix = out_dir.join("pack");
    let mut child = Command::new("git")
        .env("GIT_DIR", git_dir)
        .arg("pack-objects")
        .arg(&prefix)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run git pack-objects")?;

    let mut stdin = child.stdin.take().context("pack-objects stdin")?;
    stdin
        .write_all(&objects)
        .await
        .context("writing object list to git pack-objects")?;
    // Close stdin explicitly: pack-objects reads until EOF and without this
    // it would hang waiting for more input while we wait for its output.
    drop(stdin);

    let out = child.wait_with_output().await.context("waiting for git pack-objects")?;
    if !out.status.success() {
        bail!(
            "git pack-objects failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        bail!("git pack-objects returned no pack sha");
    }
    let base_name = format!("pack-{sha}");
    Ok(Some(PackFiles {
        pack_path: out_dir.join(format!("{base_name}.pack")),
        idx_path: out_dir.join(format!("{base_name}.idx")),
        base_name,
    }))
}

/// Base names (no extension) of the packs already present in the local
/// objects/pack.
pub async fn local_pack_names(git_dir: &Path) -> Result<HashSet<String>> {
    let dir = objects_pack_dir(git_dir);
    let mut names = HashSet::new();
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("reading {}", dir.display()))?
    {
        let file_name = entry.file_name();
        if let Some(base) = file_name.to_string_lossy().strip_suffix(".pack") {
            names.insert(base.to_string());
        }
    }
    Ok(names)
}

pub fn objects_pack_dir(git_dir: &Path) -> PathBuf {
    git_dir.join("objects").join("pack")
}

/// Full ref names (`refs/heads/...`, `refs/tags/...`) present in the repo.
pub async fn list_refs(git_dir: &Path) -> Result<Vec<String>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("for-each-ref"),
            OsStr::new("--format=%(refname)"),
            OsStr::new("refs/"),
        ],
    )
    .await?;
    Ok(out.lines().map(str::to_string).collect())
}

/// Every ref and the sha it currently points to — same set [`list_refs`]
/// lists, plus the value [`sync_bare_repo_from_s3`]-style callers need to
/// tell an already-correct ref apart from one that actually needs writing.
pub(crate) async fn list_refs_with_oids(git_dir: &Path) -> Result<HashMap<String, String>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("for-each-ref"),
            OsStr::new("--format=%(refname) %(objectname)"),
            OsStr::new("refs/"),
        ],
    )
    .await?;
    Ok(out
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(name, sha)| (name.to_string(), sha.to_string()))
        .collect())
}

/// Applies every ref create/update/delete from one manifest sync in a
/// single `git update-ref --stdin` call instead of one subprocess per ref.
/// [`crate::mirror::sync_bare_repo_from_s3`] runs on every `info/refs` and
/// `upload-pack` call (not just a push), so a repo with even a modest
/// number of branches/tags used to pay for that many subprocess spawns on
/// every single git operation — most of them writing back a value that was
/// already there, since the old code never compared before writing.
/// `updates`/`deletes` are expected already filtered down to real changes
/// (see the call site) — this only handles the "how", not the "whether".
///
/// Ref names and shas ride on stdin, not argv, so there's no
/// `--end-of-options`-style flag-injection concern the way a name starting
/// with `-` would be for a normal argument; the one thing that matters here
/// is that neither contains a newline, which would inject an extra
/// `update-ref` command into the stream — checked explicitly since these
/// values originate from a stored manifest, not a literal in this process.
pub(crate) async fn apply_ref_changes(git_dir: &Path, updates: &[(String, String)], deletes: &[String]) -> Result<()> {
    if updates.is_empty() && deletes.is_empty() {
        return Ok(());
    }
    let mut script = String::new();
    for (name, sha) in updates {
        if name.contains('\n') || sha.contains('\n') {
            bail!("ref update line contains a newline: {name:?} {sha:?}");
        }
        script.push_str("update ");
        script.push_str(name);
        script.push(' ');
        script.push_str(sha);
        script.push('\n');
    }
    for name in deletes {
        if name.contains('\n') {
            bail!("ref delete line contains a newline: {name:?}");
        }
        script.push_str("delete ");
        script.push_str(name);
        script.push('\n');
    }

    let mut child = Command::new("git")
        .env("GIT_DIR", git_dir)
        .arg("update-ref")
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run git update-ref --stdin")?;
    let mut stdin = child.stdin.take().context("update-ref --stdin stdin")?;
    stdin
        .write_all(script.as_bytes())
        .await
        .context("writing ref updates to git update-ref --stdin")?;
    drop(stdin); // `--stdin` reads until EOF, same reasoning as `cat-file --batch-check` above.

    let out = child
        .wait_with_output()
        .await
        .context("waiting for git update-ref --stdin")?;
    if !out.status.success() {
        bail!(
            "git update-ref --stdin failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub async fn update_ref(git_dir: &Path, name: &str, sha: &str) -> Result<()> {
    run(
        Some(git_dir),
        &[
            OsStr::new("update-ref"),
            OsStr::new("--end-of-options"),
            OsStr::new(name),
            OsStr::new(sha),
        ],
    )
    .await?;
    Ok(())
}

pub async fn delete_ref(git_dir: &Path, name: &str) -> Result<()> {
    run(
        Some(git_dir),
        &[
            OsStr::new("update-ref"),
            OsStr::new("-d"),
            OsStr::new("--end-of-options"),
            OsStr::new(name),
        ],
    )
    .await?;
    Ok(())
}

pub async fn set_head(git_dir: &Path, target_ref: &str) -> Result<()> {
    run(
        Some(git_dir),
        &[
            OsStr::new("symbolic-ref"),
            OsStr::new("--end-of-options"),
            OsStr::new("HEAD"),
            OsStr::new(target_ref),
        ],
    )
    .await?;
    Ok(())
}

/// `git repack -a -d`: repacks every object reachable from any ref into one
/// new pack and deletes the now-redundant old ones from the local
/// `objects/pack/` dir — used by `crate::repack` to consolidate a repo's
/// accumulated small packs (one per push, see the "no repack remoto"
/// limitation in the README) into fewer, larger ones before re-uploading.
/// Only ever run against a throwaway mirror that's just been freshly synced
/// from S3 (every object the repo actually has, nothing more): running it
/// against a mirror with in-flight local changes would be pointless (there
/// aren't any) and against one that's stale would produce a pack missing
/// objects the bucket already has.
pub async fn repack(git_dir: &Path) -> Result<()> {
    run(
        Some(git_dir),
        &[
            OsStr::new("repack"),
            OsStr::new("-a"),
            OsStr::new("-d"),
            OsStr::new("--quiet"),
        ],
    )
    .await?;
    Ok(())
}

pub async fn init_bare(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| format!("creating {}", path.display()))?;
    run(
        None,
        &[
            OsStr::new("init"),
            OsStr::new("--bare"),
            OsStr::new("--quiet"),
            OsStr::new("--end-of-options"),
            path.as_os_str(),
        ],
    )
    .await?;
    Ok(())
}
