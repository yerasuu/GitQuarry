//! Commit history: listing, pagination, message search, and per-author counts.

use crate::git::run_text;
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    pub subject: String,
    /// Refs pointing at this commit (`refs/heads/main`, `tag: v1.0`, ...),
    /// raw as `%D` gives it (no wrapping parens, unlike `%d`).
    pub decoration: String,
    /// `'N'` (no `gpgsig` header at all) short-circuits
    /// `domain::commit_signature::verified_signer` straight to `None` with
    /// no cache lookup or `gpg` spawn; `'U'` means *something* claims to be
    /// a signature, worth checking against
    /// `gitquarry_gpg::verify_detached_signature`.
    ///
    /// `'N'` until [`mark_signed`] fills it in — deliberately not carried by
    /// the `git log` walk itself, which is what `%G?` used to do at the
    /// price of one `gpg`/`ssh-keygen` fork per commit walked (see
    /// [`super::signed_shas`] for the measurements). Only the views that
    /// actually render a signature badge run that pass; a walk that just
    /// needs dates or subjects (`stats`, `last_touched`) never pays for it.
    pub signature_status: char,
}

/// Field separator for `git log`: US (unit separator) can't appear in an
/// author name or a subject, unlike `|` or `\t`.
pub(super) const LOG_SEP: &str = "\x1f";
pub(super) const LOG_FORMAT: &str = "--format=%H\x1f%an\x1f%ae\x1f%ad\x1f%s\x1f%D";

pub fn parse_log_line(line: &str) -> Option<CommitInfo> {
    let mut parts = line.splitn(6, LOG_SEP);
    Some(CommitInfo {
        sha: parts.next()?.to_string(),
        author_name: parts.next()?.to_string(),
        author_email: parts.next()?.to_string(),
        date: parts.next()?.to_string(),
        subject: parts.next().unwrap_or("").to_string(),
        decoration: parts.next().unwrap_or("").to_string(),
        signature_status: 'N',
    })
}

fn log_args<'a>(limit_flag: Option<&'a str>, rev: &'a str, path: Option<&'a str>) -> Vec<&'a OsStr> {
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("log"),
        OsStr::new(LOG_FORMAT),
        // `YYYY/MM/DD`, not the full `iso-strict` timestamp -- every table
        // that renders `CommitInfo::date` shows it as a plain date column,
        // the time-of-day/offset never mattered to any of them.
        OsStr::new("--date=format:%Y/%m/%d"),
    ];
    if let Some(flag) = limit_flag {
        args.push(OsStr::new(flag));
    }
    args.push(OsStr::new("--end-of-options"));
    args.push(OsStr::new(rev));
    let path = path.filter(|p| !p.is_empty());
    if let Some(p) = path {
        args.push(OsStr::new("--"));
        args.push(OsStr::new(p));
    }
    args
}

/// Commit history from `rev`, optionally scoped to `path`. `limit ==
/// usize::MAX` means "no cap" — `-n` takes a C `long`, so passing the
/// sentinel through as a literal `-n18446744073709551615` overflows it and
/// git rejects the whole command with "not an integer"; drop the flag instead.
pub async fn log(git_dir: &Path, rev: &str, limit: usize, path: Option<&str>) -> Result<Vec<CommitInfo>> {
    let limit_flag = (limit != usize::MAX).then(|| format!("-n{limit}"));
    let args = log_args(limit_flag.as_deref(), rev, path);
    let out = run_text(Some(git_dir), &args).await?;
    Ok(out.lines().filter_map(parse_log_line).collect())
}

/// `(subject, body)` of the commit `rev` resolves to.
///
/// Separate from [`log`]/[`CommitInfo`] because the body is the one field
/// no listing view ever wants: `%b` is multi-line by nature, and
/// [`LOG_FORMAT`] packs one commit per *line*, so it can't carry one
/// without breaking every parse. Only [`crate::domain::merge_request::create`]
/// needs it, one commit at a time.
///
/// The body comes back trimmed: `%b` keeps the trailing newlines git stores
/// after the message, which would otherwise land verbatim in an MR
/// description.
pub async fn commit_message(git_dir: &Path, rev: &str) -> Result<(String, String)> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("log"),
            OsStr::new("-n1"),
            OsStr::new("--format=%s\x1f%b"),
            OsStr::new("--end-of-options"),
            OsStr::new(rev),
        ],
    )
    .await?;
    let (subject, body) = out.split_once(LOG_SEP).unwrap_or((out.as_str(), ""));
    Ok((subject.trim().to_string(), body.trim().to_string()))
}

/// Per-commit change totals from `git log --shortstat` — cgit-style summary
/// tables show these next to each recent commit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CommitStats {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// A line like `" 3 files changed, 10 insertions(+), 2 deletions(-)"` (any
/// of the three clauses can be missing — a pure rename has no
/// insertion/deletion clause at all, a single-file change says "1 file
/// changed" not "files"). `None` for anything that isn't shaped like a
/// shortstat line at all (blank lines between log entries, mainly).
fn parse_shortstat_line(line: &str) -> Option<CommitStats> {
    let line = line.trim();
    if !line.ends_with(')') && !line.ends_with("changed") {
        return None;
    }
    let mut stats = CommitStats::default();
    let mut matched = false;
    for part in line.split(", ") {
        let part = part.trim();
        if let Some(n) = part
            .strip_suffix(" file changed")
            .or_else(|| part.strip_suffix(" files changed"))
        {
            stats.files_changed = n.trim().parse().ok()?;
            matched = true;
        } else if let Some(n) = part
            .strip_suffix(" insertion(+)")
            .or_else(|| part.strip_suffix(" insertions(+)"))
        {
            stats.insertions = n.trim().parse().ok()?;
            matched = true;
        } else {
            let n = part
                .strip_suffix(" deletion(-)")
                .or_else(|| part.strip_suffix(" deletions(-)"))?;
            stats.deletions = n.trim().parse().ok()?;
            matched = true;
        }
    }
    matched.then_some(stats)
}

/// Runs an already-built `git log` invocation and pairs each commit with
/// the `CommitStats` its `--shortstat` line reports (the caller adds that
/// flag itself — this only knows how to parse the interleaved output, not
/// where in `args` the flag belongs). `--shortstat` prints its stat line
/// right after each commit's own formatted line, so a single pass over the
/// output either extends the last-pushed entry (a shortstat line) or
/// starts a new one (a commit line).
async fn run_log_with_stats(git_dir: &Path, args: &[&OsStr]) -> Result<Vec<(CommitInfo, Option<CommitStats>)>> {
    let out = run_text(Some(git_dir), args).await?;
    let mut rows: Vec<(CommitInfo, Option<CommitStats>)> = Vec::new();
    for line in out.lines() {
        if let Some(commit) = parse_log_line(line) {
            rows.push((commit, None));
        } else if let Some(stats) = parse_shortstat_line(line)
            && let Some(last) = rows.last_mut()
        {
            last.1 = Some(stats);
        }
    }
    // Both callers of this shape (the summary tab's recent commits and the
    // log tab's own table) render a signature badge per row, so the
    // presence pass belongs here rather than duplicated in each — see
    // [`mark_signed`].
    let shas: Vec<&str> = rows.iter().map(|(c, _)| c.sha.as_str()).collect();
    let signed = super::signed_shas(git_dir, &shas).await?;
    for (commit, _) in rows.iter_mut() {
        if signed.contains(&commit.sha) {
            commit.signature_status = 'U';
        }
    }
    Ok(rows)
}

/// Same as [`log_page`] plus a `--shortstat` change total per commit, in
/// the one `git log` call rather than an extra `git show` per row — bounded
/// by `limit` regardless of repo size, same as [`log_page`] itself. Used by
/// both the summary tab's "recent commits" (`skip = 0`, `path = None`) and
/// the log tab's own paginated table.
pub async fn log_page_with_stats(
    git_dir: &Path,
    rev: &str,
    limit: usize,
    skip: usize,
    path: Option<&str>,
) -> Result<Vec<(CommitInfo, Option<CommitStats>)>> {
    let limit_flag = format!("-n{limit}");
    let skip_flag = format!("--skip={skip}");
    let mut args = log_args(Some(&limit_flag), rev, path);
    args.insert(3, OsStr::new(&skip_flag));
    args.insert(1, OsStr::new("--shortstat"));
    run_log_with_stats(git_dir, &args).await
}

/// Same as [`search_log`] plus a `--shortstat` change total per commit —
/// see [`log_page_with_stats`]'s own doc comment.
pub async fn search_log_with_stats(
    git_dir: &Path,
    rev: &str,
    query: &str,
    limit: usize,
    path: Option<&str>,
) -> Result<Vec<(CommitInfo, Option<CommitStats>)>> {
    let limit_flag = format!("-n{limit}");
    let grep = format!("--grep={query}");
    let mut args = log_args(Some(&limit_flag), rev, path);
    args.splice(3..3, [OsStr::new("-i"), OsStr::new(&grep)]);
    args.insert(1, OsStr::new("--shortstat"));
    run_log_with_stats(git_dir, &args).await
}

/// Same as [`log`] but skipping the first `skip` entries, for pagination.
pub async fn log_page(
    git_dir: &Path,
    rev: &str,
    limit: usize,
    skip: usize,
    path: Option<&str>,
) -> Result<Vec<CommitInfo>> {
    let limit_flag = format!("-n{limit}");
    let skip_flag = format!("--skip={skip}");
    // Insert right after the other flags, before `--end-of-options` / the
    // positional rev — same slot log_args leaves the format/date/limit in.
    let mut args = log_args(Some(&limit_flag), rev, path);
    args.insert(3, OsStr::new(&skip_flag));
    let out = run_text(Some(git_dir), &args).await?;
    Ok(out.lines().filter_map(parse_log_line).collect())
}

/// Total commits reachable from `rev` (optionally scoped to `path`), to know
/// whether there's a next page.
pub async fn commit_count(git_dir: &Path, rev: &str, path: Option<&str>) -> Result<usize> {
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("rev-list"),
        OsStr::new("--count"),
        OsStr::new("--end-of-options"),
        OsStr::new(rev),
    ];
    let path = path.filter(|p| !p.is_empty());
    if let Some(p) = path {
        args.push(OsStr::new("--"));
        args.push(OsStr::new(p));
    }
    let out = run_text(Some(git_dir), &args).await?;
    out.parse()
        .with_context(|| format!("commit count is not a number: {out}"))
}

/// Commits from `rev` whose message matches `query` (case-insensitive).
/// `query` must already be validated by [`crate::validate::validate_query`]: it's
/// passed straight into `--grep`.
pub async fn search_log(
    git_dir: &Path,
    rev: &str,
    query: &str,
    limit: usize,
    path: Option<&str>,
) -> Result<Vec<CommitInfo>> {
    let limit_flag = format!("-n{limit}");
    let grep = format!("--grep={query}");
    let mut args = log_args(Some(&limit_flag), rev, path);
    args.splice(3..3, [OsStr::new("-i"), OsStr::new(&grep)]);
    let out = run_text(Some(git_dir), &args).await?;
    Ok(out.lines().filter_map(parse_log_line).collect())
}

/// Commit counts per author (`git shortlog -sn`), for the stats tab.
pub async fn shortlog_counts(git_dir: &Path, rev: &str) -> Result<Vec<(u64, String)>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("shortlog"),
            OsStr::new("-sn"),
            OsStr::new("--end-of-options"),
            OsStr::new(rev),
        ],
    )
    .await?;
    Ok(parse_shortlog(&out))
}

pub fn parse_shortlog(text: &str) -> Vec<(u64, String)> {
    let mut counts = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if let Some((n, author)) = line.split_once('\t')
            && let Ok(n) = n.trim().parse()
        {
            counts.push((n, author.to_string()));
        }
    }
    counts
}

/// Fills in [`CommitInfo::signature_status`] for a whole listing in one
/// `git cat-file --batch` — the pass the `git log` walk deliberately no
/// longer does inline (see that field's own doc comment). Called only by
/// the views that render a signature badge; everything else leaves every
/// row at `'N'` and skips the subprocess entirely.
///
/// `'U'` rather than a git-side grade: what a badge ultimately shows comes
/// from [`crate::domain::commit_signature::verified_signer`] checking the
/// signature against the accounts' own registered keys, and "there is a
/// signature here" is the entire input that step needs.
pub async fn mark_signed(git_dir: &Path, commits: &mut [CommitInfo]) -> Result<()> {
    let shas: Vec<&str> = commits.iter().map(|c| c.sha.as_str()).collect();
    let signed = super::signed_shas(git_dir, &shas).await?;
    for commit in commits.iter_mut() {
        if signed.contains(&commit.sha) {
            commit.signature_status = 'U';
        }
    }
    Ok(())
}
