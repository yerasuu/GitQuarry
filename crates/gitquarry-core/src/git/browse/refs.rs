//! Branch/tag listing for the refs and summary tabs.

use super::log::LOG_SEP;
use crate::git::run_text;
use anyhow::Result;
use std::ffi::OsStr;
use std::path::Path;

#[derive(Debug)]
pub struct RefInfo {
    pub name: String,
    pub sha: String,
    /// The ref's own subject line (`%(subject)`, no `*` prefix): for an
    /// annotated tag that's the tag message itself, for a lightweight tag
    /// or a branch it's the pointed-at commit's subject. Using the `*`
    /// (dereferenced) form instead would swap that — an annotated tag would
    /// show its *commit's* subject rather than the tag message, which is
    /// backwards for a refs listing.
    pub subject: String,
    pub date: String,
    /// `%(authorname)` for a branch/lightweight tag (the pointed-at
    /// commit's author) or `%(taggername)` for an annotated tag — same
    /// dereference-vs-not split `subject`'s own doc comment already
    /// explains, so an annotated tag credits whoever tagged it, not
    /// whoever wrote the underlying commit.
    pub author_name: String,
    /// Same tagger-vs-author dereference as `author_name`, trimmed of the
    /// `<...>` angle brackets `%(authoremail)` normally carries — matches
    /// the bare-email shape `git log`'s `%ae` gives [`super::log::CommitInfo::author_email`],
    /// so both feed the same [`super::super::super::domain::commit_signature`]/
    /// email-match avatar lookup.
    pub author_email: String,
    /// `%(signature:grade)`, same grade letters as [`super::log::CommitInfo::signature_status`]'s
    /// `%G?` (`N` = unsigned) — lets a signed branch/tag tip resolve to a
    /// GPG-verified avatar the same way a signed commit does, instead of
    /// only ever falling back to the weaker email match.
    pub signature_status: char,
}

// pub(crate) visibility isn't enough for the integration test in
// tests/git_browse_refs.rs, and this is also used unconditionally by
// list_refs_detailed below, so it can't be cfg-gated to test builds only.
pub fn parse_ref_line(line: &str) -> Option<RefInfo> {
    let mut parts = line.splitn(7, LOG_SEP);
    Some(RefInfo {
        name: parts.next()?.to_string(),
        sha: parts.next()?.to_string(),
        subject: parts.next().unwrap_or("").to_string(),
        date: parts.next().unwrap_or("").to_string(),
        author_name: parts.next().unwrap_or("").to_string(),
        author_email: parts.next().unwrap_or("").to_string(),
        signature_status: parts.next().and_then(|s| s.chars().next()).unwrap_or('N'),
    })
}

/// Branches and tags with enough detail to render the refs/summary tabs.
pub async fn list_refs_detailed(git_dir: &Path, pattern: &str) -> Result<Vec<RefInfo>> {
    // `%(taggername)` only resolves for an annotated tag object (empty for
    // a branch/lightweight tag, which points straight at a commit); falls
    // back to `%(authorname)` for those — same dereference-vs-not split
    // `RefInfo::author_name`'s own doc comment explains.
    // `format:%Y/%m/%d`, not `iso-strict` -- same plain-date display
    // `log_args` uses, see its own doc comment.
    let format = format!(
        "--format=%(refname){LOG_SEP}%(objectname){LOG_SEP}%(subject){LOG_SEP}%(creatordate:format:%Y/%m/%d){LOG_SEP}%(if)%(taggername)%(then)%(taggername)%(else)%(authorname)%(end){LOG_SEP}%(if)%(taggername)%(then)%(taggeremail:trim)%(else)%(authoremail:trim)%(end){LOG_SEP}%(signature:grade)"
    );
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("for-each-ref"),
            OsStr::new("--sort=-creatordate"),
            OsStr::new(&format),
            OsStr::new(pattern),
        ],
    )
    .await?;
    Ok(out.lines().filter_map(parse_ref_line).collect())
}

/// Just branch names, sorted — the cgit-style ref switcher (repo header,
/// every view) needs a name to link each branch, nothing [`list_refs_detailed`]
/// otherwise fetches (sha/subject/date), and paying for `%(creatordate)` on
/// every single page view (not just the summary/refs tabs) would be wasteful
/// for a dropdown that only ever shows the name.
pub async fn branch_names(git_dir: &Path) -> Result<Vec<String>> {
    let out = run_text(
        Some(git_dir),
        &[
            OsStr::new("for-each-ref"),
            OsStr::new("--format=%(refname:short)"),
            OsStr::new("--sort=refname"),
            OsStr::new("refs/heads/"),
        ],
    )
    .await?;
    Ok(out.lines().map(str::to_string).collect())
}
