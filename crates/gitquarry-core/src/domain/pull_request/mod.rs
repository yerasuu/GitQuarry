//! Every mutation of a [`crate::storage::pull_requests::PullRequestMeta`]
//! goes through here — same `domain`/`web` boundary [`crate::domain::repo`]'s
//! doc comment describes: transport-agnostic, `anyhow::Result`, no CSRF/
//! session/audit concerns, those stay in `webui::repo::pulls`.
//!
//! A PR can be same-repo (`source_repo == target.uid`) or cross-fork
//! (`source_repo` is one of the target's forks — `RepoConfig::forked_from`
//! must point back at the target, never an arbitrary unrelated repo). A
//! cross-fork PR still lives entirely on the *target*'s own storage, same as
//! a GitHub PR belongs to the upstream repo, not the fork.
//!
//! Forks are fully independent S3 prefixes and bare mirrors (`domain::repo::fork`,
//! `storage::remote::Remote::copy_all_to` — a one-time full re-encrypt copy,
//! nothing shared afterward) — there's no other path for the target's mirror
//! to see a fork's objects, so [`diff`]/[`merge`] fetch them on demand via
//! [`crate::git::fetch_local`] into a scratch ref, never persisted to the
//! target's own manifest (see [`fetch_source_ref`]'s own doc comment).

use crate::app::{AppContext, ResolvedRepo};
use crate::auth::access;
use crate::git;
use crate::storage::Cas;
use crate::storage::audit::format_unix_secs;
use crate::storage::pull_requests::{
    self, CommentAnchor, MergeStrategy, PrComment, PrReview, PrStatus, PullRequestMeta, ReviewVerdict,
};
use crate::storage::remote::Remote;
use crate::storage::uid::Uid;
use anyhow::{Result, bail};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_secs(secs)
}

/// `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE` want `<unix-seconds> <tz-offset>`,
/// not the ISO 8601 shape [`now_stamp`] gives every other timestamp in this
/// module — always UTC here, there's no per-request timezone to thread in.
fn git_date_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs} +0000")
}

/// Resolves `username` to a `(name, email)` git identity for a merge commit.
///
/// An account with no `email` (allowed at signup, and the *only* thing
/// `setup.rs`'s bootstrap admin can be) gets a synthetic `users.noreply`
/// address rather than failing the merge or leaving it to `git`'s own
/// config, which a bare deploy container never has
/// ([`commit_tree`](git::commit_tree) always wants both explicit).
///
/// A backend *error* is NOT that case and must not be collapsed into it.
/// This used to `.ok()` the read away, so a transient failure fetching the
/// user stamped the merge commit `<username>@users.noreply.gitquarry` and
/// moved on — permanently misattributed, and indistinguishable afterward
/// from an account that genuinely has no email. Nothing has been written
/// yet when this is called, so failing here just means a retry; a bad
/// identity in a commit object is forever.
async fn merger_identity(ctx: &AppContext, username: &str) -> Result<(String, String)> {
    // No local account is a legitimate case, not an error: a merge proxied
    // in over the loopback API names whoever
    // merged it on *their* instance, who by definition has no `User` here.
    // Same noreply treatment as a local account without an email — both are
    // "there is no address to use", which is exactly what that synthetic
    // domain is for.
    let (user, _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?;
    let email = user
        .and_then(|u| u.email)
        .unwrap_or_else(|| format!("{username}@users.noreply.gitquarry"));
    Ok((username.to_string(), email))
}

async fn branch_exists(remote: &Remote, branch: &str) -> Result<bool> {
    let (manifest, _etag) = remote.get_manifest().await?;
    Ok(manifest.refs.contains_key(&format!("refs/heads/{branch}")))
}

pub struct NewPullRequest<'a> {
    pub source_repo: &'a Uid,
    pub source_branch: &'a str,
    pub target_branch: &'a str,
    pub title: &'a str,
    pub description: &'a str,
    pub author: &'a str,
}

/// The title and description to store, filling either in from the source
/// branch's tip commit when the submitted one is blank: its subject becomes
/// the title, its body the description — the same thing a `git request-pull`
/// or a one-commit GitHub PR does, and the only description a PR
/// necessarily already has.
///
/// The title is also *validated* here, which [`create`] never did before —
/// only [`edit`] ran `validate_issue_title`, so a PR could be created with
/// an empty (or control-character-laden) title and only become
/// unrepresentable once someone tried to edit it. Falling back to the
/// commit subject means that no longer needs to be an error in the common
/// case: the only way to reach one now is a branch whose tip commit has an
/// empty subject too.
///
/// A blank description stays blank when the tip commit has no body — unlike
/// the title, an empty description is a perfectly ordinary PR.
async fn resolve_title_and_description(
    ctx: &AppContext,
    target: &ResolvedRepo,
    source_remote: &Remote,
    new: &NewPullRequest<'_>,
    locale: gitquarry_i18n::Locale,
) -> Result<(String, String)> {
    let title = new.title.trim();
    let description = new.description;
    if !title.is_empty() && !description.trim().is_empty() {
        return Ok((crate::validate::validate_issue_title(title)?, description.to_string()));
    }

    // Only opened when something is actually missing — a fully filled-in
    // form shouldn't pay for syncing a mirror. Safe to open the target's
    // own here: unlike `diff`/`merge`, `create` is reached from the
    // top-level `/_pulls` POST route, which holds no mirror lock (see
    // `diff`'s doc comment on why that distinction matters).
    let source = ResolvedRepo {
        uid: new.source_repo.clone(),
        remote: source_remote.clone(),
    };
    let mirror = if new.source_repo == &target.uid {
        ctx.open_mirror(target).await?
    } else {
        ctx.open_mirror(&source).await?
    };
    let (subject, body) = git::browse::commit_message(&mirror.path, new.source_branch).await?;

    let title = if title.is_empty() { subject.trim() } else { title };
    let description = if description.trim().is_empty() {
        body
    } else {
        description.to_string()
    };
    if title.is_empty() {
        bail!("{}", gitquarry_i18n::shared::pull_request_needs_a_title(locale));
    }
    Ok((crate::validate::validate_issue_title(title)?, description))
}

/// `Err` if `new.source_repo` isn't the target itself or one of its forks
/// (`RepoConfig::forked_from` pointing back at `target.uid`), or if
/// `source_branch`/`target_branch` aren't actual branches on their
/// respective repos.
pub async fn create(
    ctx: &AppContext,
    target: &ResolvedRepo,
    new: NewPullRequest<'_>,
    locale: gitquarry_i18n::Locale,
) -> Result<PullRequestMeta> {
    let source_remote = if new.source_repo == &target.uid {
        target.remote.clone()
    } else {
        let source_remote = ctx.remote_for_uid(new.source_repo);
        let (source_config, etag) = source_remote.get_repo_config().await?;
        if etag.is_none() {
            bail!(
                "{}",
                gitquarry_i18n::shared::repo_not_found(locale, new.source_repo.as_str())
            );
        }
        if source_config.forked_from.as_ref() != Some(&target.uid) {
            bail!("{}", gitquarry_i18n::shared::source_repo_not_a_fork(locale));
        }
        source_remote
    };
    if !branch_exists(&source_remote, new.source_branch).await? {
        bail!("{}", gitquarry_i18n::shared::not_a_branch(locale, new.source_branch));
    }
    if !branch_exists(&target.remote, new.target_branch).await? {
        bail!("{}", gitquarry_i18n::shared::not_a_branch(locale, new.target_branch));
    }

    let (title, description) = resolve_title_and_description(ctx, target, &source_remote, &new, locale).await?;

    let backend = ctx.backend();
    let number = pull_requests::next_pr_number(backend.as_ref(), &ctx.cipher, &target.uid).await?;
    let stamp = now_stamp();
    let pr = PullRequestMeta {
        number,
        title,
        description,
        source_repo: new.source_repo.clone(),
        source_branch: new.source_branch.to_string(),
        target_branch: new.target_branch.to_string(),
        status: PrStatus::Open,
        author: new.author.to_string(),
        created_at: stamp.clone(),
        updated_at: stamp,
        merged_at: None,
        merged_by: None,
        merge_commit_sha: None,
        is_draft: false,
        labels: Vec::new(),
        assignees: Vec::new(),
        requested_reviewers: Vec::new(),
    };
    let cas = pull_requests::put_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, &pr, None).await?;
    if cas != Cas::Ok {
        bail!("PR number collision — the counter is supposed to guarantee this never happens");
    }
    crate::events::emit(
        ctx,
        crate::events::RepoEvent::PullRequest {
            repo_uid: target.uid.clone(),
            number: pr.number,
            action: crate::events::PullRequestAction::Opened,
        },
    );
    Ok(pr)
}

pub async fn get(ctx: &AppContext, target: &ResolvedRepo, number: u64) -> Result<Option<PullRequestMeta>> {
    let (pr, _etag) = pull_requests::get_pull_request(ctx.backend().as_ref(), &ctx.cipher, &target.uid, number).await?;
    Ok(pr)
}

/// Every PR on `target`, newest-first, optionally narrowed to one status.
pub async fn list(
    ctx: &AppContext,
    target: &ResolvedRepo,
    status_filter: Option<PrStatus>,
) -> Result<Vec<PullRequestMeta>> {
    let backend = ctx.backend();
    let mut prs = pull_requests::list_pull_requests(&backend, &ctx.cipher, &target.uid).await?;
    if let Some(status) = status_filter {
        prs.retain(|pr| pr.status == status);
    }
    Ok(prs)
}

/// `Err` if `number` doesn't exist or isn't currently in `expected` status —
/// no reopening a merged PR, no closing an already-closed one.
async fn transition(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    expected: PrStatus,
    next: PrStatus,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let backend = ctx.backend();
    let (pr, etag) = pull_requests::get_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, number).await?;
    let Some(mut pr) = pr else {
        bail!("{}", gitquarry_i18n::shared::pull_request_not_found(locale, number));
    };
    if pr.status != expected {
        bail!("{}", gitquarry_i18n::shared::pull_request_wrong_status(locale));
    }
    pr.status = next;
    pr.updated_at = now_stamp();
    pull_requests::put_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, &pr, etag.as_deref()).await
}

pub async fn close(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let cas = transition(ctx, target, number, PrStatus::Open, PrStatus::Closed, locale).await?;
    if cas == Cas::Ok {
        crate::events::emit(
            ctx,
            crate::events::RepoEvent::PullRequest {
                repo_uid: target.uid.clone(),
                number,
                action: crate::events::PullRequestAction::Closed,
            },
        );
    }
    Ok(cas)
}

pub async fn reopen(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    transition(ctx, target, number, PrStatus::Closed, PrStatus::Open, locale).await
}

/// `Err` if `number` doesn't exist, isn't `Open`, or is already in the
/// requested draft state — same "no-op transition rejected" shape
/// [`transition`] gives `close`/`reopen`, just keyed on `is_draft` instead
/// of `status` since a draft PR is still `Open`, not a third status.
async fn set_draft(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    draft: bool,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let backend = ctx.backend();
    let (pr, etag) = pull_requests::get_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, number).await?;
    let Some(mut pr) = pr else {
        bail!("{}", gitquarry_i18n::shared::pull_request_not_found(locale, number));
    };
    if pr.status != PrStatus::Open || pr.is_draft == draft {
        bail!("{}", gitquarry_i18n::shared::pull_request_wrong_status(locale));
    }
    pr.is_draft = draft;
    pr.updated_at = now_stamp();
    pull_requests::put_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, &pr, etag.as_deref()).await
}

pub async fn mark_draft(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    set_draft(ctx, target, number, true, locale).await
}

pub async fn mark_ready(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    set_draft(ctx, target, number, false, locale).await
}

/// Bundled so [`edit_metadata`] doesn't cross clippy's `too_many_arguments`
/// — same reasoning as [`NewPullRequest`]. All three are comma-separated
/// free text, same shape every other CSV-ish field in this codebase
/// (`domain::issue::IssueEdit`, `topics`, mirror scopes) takes at the edge.
pub struct PrMetadataEdit<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub labels_csv: &'a str,
    pub assignees_csv: &'a str,
    pub reviewers_csv: &'a str,
}

/// Title, description, labels, assignees, and requested reviewers all
/// change together in a single get-etag/mutate/CAS-put. Every label in
/// `edit.labels_csv` must already exist in `target`'s
/// `RepoConfig.labels`, every name in `assignees_csv`/`reviewers_csv` must
/// resolve to a real user — the first one that doesn't bails with a clear
/// error naming it, before anything is written.
pub async fn edit_metadata(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    edit: PrMetadataEdit<'_>,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let title = crate::validate::validate_issue_title(edit.title)?;
    let (config, _etag) = target.remote.get_repo_config().await?;
    let known_labels: std::collections::BTreeSet<&str> = config.labels.iter().map(|l| l.name.as_str()).collect();
    let mut labels = Vec::new();
    for name in edit.labels_csv.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if !known_labels.contains(name) {
            bail!("{}", gitquarry_i18n::shared::unknown_label(locale, name));
        }
        labels.push(name.to_string());
    }

    let backend = ctx.backend();
    let mut assignees = Vec::new();
    for username in edit.assignees_csv.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let (user, _etag) = access::get_user(backend.as_ref(), &ctx.cipher, username).await?;
        if user.is_none() {
            bail!("{}", gitquarry_i18n::shared::unknown_assignee(locale, username));
        }
        assignees.push(username.to_string());
    }
    let mut reviewers = Vec::new();
    for username in edit.reviewers_csv.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let (user, _etag) = access::get_user(backend.as_ref(), &ctx.cipher, username).await?;
        if user.is_none() {
            bail!("{}", gitquarry_i18n::shared::unknown_reviewer(locale, username));
        }
        reviewers.push(username.to_string());
    }

    let (pr, etag) = pull_requests::get_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, number).await?;
    let Some(mut pr) = pr else {
        bail!("{}", gitquarry_i18n::shared::pull_request_not_found(locale, number));
    };
    pr.title = title;
    pr.description = edit.description.to_string();
    pr.labels = labels;
    pr.assignees = assignees;
    pr.requested_reviewers = reviewers;
    pr.updated_at = now_stamp();
    let cas = pull_requests::put_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, &pr, etag.as_deref()).await?;
    if cas == Cas::Ok {
        crate::events::emit(
            ctx,
            crate::events::RepoEvent::PullRequest {
                repo_uid: target.uid.clone(),
                number,
                action: crate::events::PullRequestAction::Updated,
            },
        );
    }
    Ok(cas)
}

/// `Err` if `number` doesn't exist — a comment can't be left on a PR that
/// was never created (or was, somewhere else, but not on this repo).
pub async fn add_comment(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    author: &str,
    body: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    add_comment_anchored(ctx, target, number, author, body, None, locale).await
}

/// [`add_comment`], but hung off one diff line instead of the flat thread.
/// The anchor is only checked for being well-formed (a path, a 1-based
/// line) — never for actually appearing in the PR's current diff: a diff is
/// recomputed per request against a moving head, so a "does this line
/// exist" check would be a race, and the render side already falls back to
/// the flat thread for an anchor it can't place.
pub async fn add_line_comment(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    author: &str,
    body: &str,
    anchor: CommentAnchor,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    if anchor.path.is_empty() || anchor.line == 0 {
        bail!("{}", gitquarry_i18n::shared::invalid_comment_anchor(locale));
    }
    add_comment_anchored(ctx, target, number, author, body, Some(anchor), locale).await
}

async fn add_comment_anchored(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    author: &str,
    body: &str,
    anchor: Option<CommentAnchor>,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    if get(ctx, target, number).await?.is_none() {
        bail!("{}", gitquarry_i18n::shared::pull_request_not_found(locale, number));
    }
    let comment = PrComment {
        author: author.to_string(),
        body: body.to_string(),
        created_at: now_stamp(),
        anchor,
    };
    pull_requests::add_pr_comment(ctx.backend().as_ref(), &ctx.cipher, &target.uid, number, &comment).await
}

/// Oldest first, each paired with the storage key `edit_comment`/
/// `delete_comment` need to target it — same shape
/// `domain::issue::list_comments` already returns.
pub async fn list_comments(ctx: &AppContext, target: &ResolvedRepo, number: u64) -> Result<Vec<(String, PrComment)>> {
    let backend = ctx.backend();
    pull_requests::list_pr_comments_with_keys(&backend, &ctx.cipher, &target.uid, number).await
}

/// `Err` if `comment_id` doesn't exist or `actor` isn't the comment's own
/// author — same author-only bar as `domain::issue::edit_comment`, no
/// `Developer`+ override.
pub async fn edit_comment(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    comment_id: &str,
    actor: &str,
    body: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let backend = ctx.backend();
    let (comment, etag) =
        pull_requests::get_pr_comment(backend.as_ref(), &ctx.cipher, &target.uid, number, comment_id).await?;
    let Some(mut comment) = comment else {
        bail!("comment {comment_id} not found on PR #{number}");
    };
    if comment.author != actor {
        bail!("{}", gitquarry_i18n::shared::not_comment_author(locale));
    }
    comment.body = body.to_string();
    pull_requests::put_pr_comment(
        backend.as_ref(),
        &ctx.cipher,
        &target.uid,
        number,
        comment_id,
        &comment,
        etag.as_deref(),
    )
    .await
}

/// Same author-only bar as [`edit_comment`] — *unless* `is_moderator`, same
/// `Developer`+ override `domain::issue::delete_comment` now has (see that
/// function's own doc comment for why delete gets one and edit doesn't).
pub async fn delete_comment(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    comment_id: &str,
    actor: &str,
    is_moderator: bool,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    let backend = ctx.backend();
    let (comment, _etag) =
        pull_requests::get_pr_comment(backend.as_ref(), &ctx.cipher, &target.uid, number, comment_id).await?;
    let Some(comment) = comment else {
        bail!("comment {comment_id} not found on PR #{number}");
    };
    if comment.author != actor && !is_moderator {
        bail!("{}", gitquarry_i18n::shared::not_comment_author(locale));
    }
    pull_requests::delete_pr_comment(backend.as_ref(), &ctx.cipher, &target.uid, number, comment_id).await;
    Ok(())
}

/// `Err` if `number` doesn't exist, or if `author` is the PR's own author
/// submitting anything but the neutral [`ReviewVerdict::Commented`] — you
/// don't get to approve your own work.
pub async fn add_review(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    author: &str,
    verdict: ReviewVerdict,
    body: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    let Some(pr) = get(ctx, target, number).await? else {
        bail!("{}", gitquarry_i18n::shared::pull_request_not_found(locale, number));
    };
    if pr.author == author && verdict != ReviewVerdict::Commented {
        bail!("{}", gitquarry_i18n::shared::cannot_review_own_pull_request(locale));
    }
    let review = PrReview {
        author: author.to_string(),
        verdict,
        body: body.to_string(),
        created_at: now_stamp(),
    };
    pull_requests::add_pr_review(ctx.backend().as_ref(), &ctx.cipher, &target.uid, number, &review).await
}

pub async fn list_reviews(ctx: &AppContext, target: &ResolvedRepo, number: u64) -> Result<Vec<PrReview>> {
    let backend = ctx.backend();
    pull_requests::list_pr_reviews(&backend, &ctx.cipher, &target.uid, number).await
}

/// Who currently approves and who currently requests changes, both sorted
/// by username for a stable render.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReviewSummary {
    pub approved_by: Vec<String>,
    pub changes_requested_by: Vec<String>,
}

/// Collapses a review thread to its current state: only each reviewer's
/// *latest* review counts, so someone who requested changes and later
/// approved shows up as an approval only — same semantics GitHub's own
/// review state has. A [`ReviewVerdict::Commented`] review counts for
/// neither side, but still supersedes that reviewer's earlier verdict:
/// it's an explicit "I no longer hold a position", not a no-op.
pub fn review_summary(reviews: &[PrReview]) -> ReviewSummary {
    let mut latest: std::collections::HashMap<&str, ReviewVerdict> = std::collections::HashMap::new();
    for review in reviews {
        latest.insert(review.author.as_str(), review.verdict);
    }

    let mut summary = ReviewSummary::default();
    for (author, verdict) in latest {
        match verdict {
            ReviewVerdict::Approved => summary.approved_by.push(author.to_string()),
            ReviewVerdict::ChangesRequested => summary.changes_requested_by.push(author.to_string()),
            ReviewVerdict::Commented => {}
        }
    }
    summary.approved_by.sort_unstable();
    summary.changes_requested_by.sort_unstable();
    summary
}

pub struct PrDiff {
    pub stat: String,
    pub diff: String,
    pub commits: Vec<git::browse::CommitInfo>,
}

/// Scratch ref a cross-fork PR's source objects land under, in the
/// *target*'s mirror — namespaced under `refs/quarry/` so it can never
/// collide with a real branch/tag the target repo's own manifest tracks.
fn shadow_ref(number: u64) -> String {
    format!("refs/quarry/pr/{number}")
}

/// For a same-repo PR, `pr.source_branch` is already a local ref in
/// `git_dir` — no fetch needed. For a cross-fork one, opens the fork's own
/// mirror (a different repo uid, so this can't deadlock against the
/// target's mirror lock the caller already holds — see [`diff`]'s own doc
/// comment on that) and [`crate::git::fetch_local`]s `source_branch` into
/// `git_dir` under [`shadow_ref`], returning that ref's name to diff/merge
/// against instead. Never written to the target's own manifest — it's
/// scratch, fine that the next `mirror::sync_bare_repo_from_s3` deletes it;
/// re-fetching is idempotent (already-present objects are skipped).
async fn fetch_source_ref(
    ctx: &AppContext,
    target: &ResolvedRepo,
    git_dir: &std::path::Path,
    pr: &PullRequestMeta,
) -> Result<String> {
    if pr.source_repo == target.uid {
        return Ok(pr.source_branch.clone());
    }
    let source = ResolvedRepo {
        uid: pr.source_repo.clone(),
        remote: ctx.remote_for_uid(&pr.source_repo),
    };
    let source_mirror = ctx.open_mirror(&source).await?;
    let local_ref = shadow_ref(pr.number);
    git::fetch_local(git_dir, &source_mirror.path, &pr.source_branch, &local_ref).await?;
    Ok(local_ref)
}

/// The diff/commit-list a PR proposes: `target_branch` (base) vs
/// `source_branch` (head, fetched via [`fetch_source_ref`] if cross-fork),
/// same three-dot/two-dot convention `webui::repo::compare::render_compare`
/// uses for the standalone compare view. Takes the *target*'s already-open
/// mirror's `git_dir` rather than opening one itself — `webui::repo::handler_repo`
/// already holds the target repo's mirror lock for the whole request by the
/// time it reaches a `"pull"` view, and `AppContext::open_mirror`'s lock
/// isn't reentrant: a second call on the *target* from in here would
/// deadlock against the caller's own guard (opening the *source* fork's
/// mirror is fine — different uid, different lock).
pub async fn diff(
    ctx: &AppContext,
    target: &ResolvedRepo,
    git_dir: &std::path::Path,
    pr: &PullRequestMeta,
) -> Result<PrDiff> {
    let base = pr.target_branch.as_str();
    let head = fetch_source_ref(ctx, target, git_dir, pr).await?;
    let stat = git::browse::diffstat_range(git_dir, base, &head).await?;
    let diff = git::browse::diff_range(git_dir, base, &head).await?;
    let range = format!("{base}..{head}");
    let commits = git::browse::log(git_dir, &range, usize::MAX, None).await?;
    Ok(PrDiff { stat, diff, commits })
}

/// Whether this PR would merge cleanly right now: `None` for a clean
/// merge, `Some(paths)` for the files that conflict. Dry-run only —
/// `git merge-tree` writes nothing beyond git's own scratch objects, so
/// unlike [`merge`] this can be called on a plain page render.
///
/// Deliberately probes the `Merge` strategy alone even though a PR can be
/// merged three ways: `Squash` computes the identical tree (it differs
/// only in how many parents the commit gets), and `Rebase` replays commit
/// by commit, so a conflict there depends on which commit you stop at.
/// Reporting "merge and squash would conflict here" is honest for two of
/// the three and never a false *clean* for the third — a rebase that
/// conflicts always has at least one conflicting path in this set too.
///
/// Errors like a deleted target branch surface as `Ok(None)`, not `Err`:
/// this only ever decorates a page, and a merge attempt reports the real
/// reason properly. A view must not 500 because a probe couldn't run.
pub async fn conflicts(
    ctx: &AppContext,
    target: &ResolvedRepo,
    git_dir: &std::path::Path,
    pr: &PullRequestMeta,
) -> Result<Option<Vec<String>>> {
    let head_ref = fetch_source_ref(ctx, target, git_dir, pr).await?;
    let (manifest, _etag) = target.remote.get_manifest().await?;
    let Some(target_sha) = manifest.refs.get(&format!("refs/heads/{}", pr.target_branch)).cloned() else {
        return Ok(None);
    };
    let Some(head_sha) = git::browse::resolve_commit(git_dir, &head_ref).await? else {
        return Ok(None);
    };
    match git::merge_tree(git_dir, &target_sha, &head_sha, None).await {
        Ok(git::MergeTreeOutcome::Conflicted(paths)) => Ok(Some(paths)),
        Ok(git::MergeTreeOutcome::Clean(_)) => Ok(None),
        Err(_) => Ok(None),
    }
}

pub enum MergeOutcome {
    Merged(String),
    Conflict(Vec<String>),
}

/// Computes the final sha to fast-forward the target branch to, per
/// `strategy`. Doesn't touch the manifest or the PR's own metadata — [`merge`]
/// does that once for all three strategies, since it's identical either way.
///
/// - `Merge`: a real 3-way merge commit with both `target_sha`/`head_sha` as
///   parents, same as today.
/// - `Squash`: the same 3-way merge tree, but a single-parent commit (just
///   `target_sha`) — the source branch's own commits never become ancestors
///   of the target, only their combined diff does. Message lists each
///   squashed commit's subject under the PR title, same shape GitHub's
///   squash-merge produces.
/// - `Rebase`: replays each source commit onto the target tip one at a time
///   via [`git::merge_tree`]'s `merge_base` override (cherry-pick
///   semantics), preserving each commit's own author/date. Bails if any
///   replayed commit is itself a merge commit or a root commit — a linear
///   PR branch never hits either, and guessing a merge base for one would
///   silently replay the wrong diff (see [`git::RebaseCommit`]'s doc
///   comment).
async fn merge_commit(
    git_dir: &std::path::Path,
    pr: &PullRequestMeta,
    target_sha: &str,
    head_sha: &str,
    strategy: MergeStrategy,
    merger: (&str, &str),
) -> Result<MergeOutcome> {
    let merge_date = git_date_now();
    let merger_ident = (merger.0, merger.1, merge_date.as_str());
    match strategy {
        MergeStrategy::Merge => {
            let tree = match git::merge_tree(git_dir, target_sha, head_sha, None).await? {
                git::MergeTreeOutcome::Conflicted(paths) => return Ok(MergeOutcome::Conflict(paths)),
                git::MergeTreeOutcome::Clean(tree) => tree,
            };
            let message = format!(
                "Merge pull request #{} from {}\n\n{}",
                pr.number, pr.source_branch, pr.title
            );
            let sha = git::commit_tree(
                git_dir,
                &tree,
                &[target_sha, head_sha],
                &message,
                merger_ident,
                merger_ident,
            )
            .await?;
            Ok(MergeOutcome::Merged(sha))
        }
        MergeStrategy::Squash => {
            let tree = match git::merge_tree(git_dir, target_sha, head_sha, None).await? {
                git::MergeTreeOutcome::Conflicted(paths) => return Ok(MergeOutcome::Conflict(paths)),
                git::MergeTreeOutcome::Clean(tree) => tree,
            };
            let range = format!("{target_sha}..{head_sha}");
            let mut commits = git::browse::log(git_dir, &range, usize::MAX, None).await?;
            commits.reverse(); // browse::log is newest-first; a squash body reads oldest-first
            let mut message = format!("{} (#{})\n", pr.title, pr.number);
            for commit in &commits {
                message.push_str(&format!("\n* {}", commit.subject));
            }
            let sha = git::commit_tree(git_dir, &tree, &[target_sha], &message, merger_ident, merger_ident).await?;
            Ok(MergeOutcome::Merged(sha))
        }
        MergeStrategy::Rebase => {
            let range = format!("{target_sha}..{head_sha}");
            let shas = git::rev_list_range(git_dir, &range).await?;
            let mut new_tip = target_sha.to_string();
            for sha in shas {
                let commit = git::commit_detail_for_rebase(git_dir, &sha).await?;
                let [parent] = commit.parents.as_slice() else {
                    bail!(
                        "can't rebase pull request #{}: commit {sha} isn't a single-parent commit (merge or root commit in the source branch)",
                        pr.number
                    );
                };
                let tree = match git::merge_tree(git_dir, &new_tip, &sha, Some(parent)).await? {
                    git::MergeTreeOutcome::Conflicted(paths) => return Ok(MergeOutcome::Conflict(paths)),
                    git::MergeTreeOutcome::Clean(tree) => tree,
                };
                new_tip = git::commit_tree(
                    git_dir,
                    &tree,
                    &[&new_tip],
                    &commit.message,
                    (&commit.author_name, &commit.author_email, &commit.author_date),
                    merger_ident,
                )
                .await?;
            }
            Ok(MergeOutcome::Merged(new_tip))
        }
    }
}

/// Merges `number` into its target branch per `strategy` (or `Conflict`
/// with the conflicting paths, nothing written), then uploaded and
/// CAS-linked into the target's own manifest exactly the way a real push
/// would — reusing [`git::pack_objects_for_push`]/[`Remote::upload_pack`]
/// rather than inventing separate upload plumbing. See [`merge_commit`] for
/// how each strategy computes its final sha.
///
/// Unlike [`diff`], this opens the target's mirror itself: called directly
/// from a `POST /_pulls/merge` request, not nested inside a `handler_repo`
/// GET view that already holds it, so there's no reentrant-lock risk here.
///
/// `Err` if `number` doesn't exist, isn't `Open`, is still a draft, has a
/// reviewer's latest verdict at [`ReviewVerdict::ChangesRequested`], or the
/// target branch has since been deleted; a merge race (the target branch
/// moved between reading its manifest and the CAS write below) surfaces as
/// [`gitquarry_i18n::shared::cas_conflict`] — same "reload and retry" contract
/// every other CAS writer in this codebase already gives. The review-state
/// gate only checks for a pending *rejection* — it doesn't require any
/// approvals to exist, unlike a GitHub branch-protection rule.
/// The `CODEOWNERS` file location this codebase checks — repo root only,
/// unlike GitHub's own fallback chain (`.github/CODEOWNERS`,
/// `docs/CODEOWNERS`) — one location is enough for what this feature needs
/// to cover.
const CODEOWNERS_PATH: &str = "CODEOWNERS";

/// `None` if every required approver (the repo's own `RepoConfig::required_reviewers`
/// plus whoever `CODEOWNERS` names for a path this PR actually touches) has
/// approved; `Some(missing)` — sorted, deduped — otherwise. The PR's own
/// author is never counted as "missing" here even if named: self-approval
/// is already blocked elsewhere ([`add_review`]), so requiring it would
/// make such a PR permanently unmergeable. `CODEOWNERS` is read from the
/// *target* branch's tree at `target_sha` (the state this PR would land
/// against), same "target's rules apply" convention GitHub's own feature
/// uses — a missing file just means no path-based requirements, not an
/// error.
async fn missing_required_reviewers(
    target: &ResolvedRepo,
    git_dir: &Path,
    pr: &PullRequestMeta,
    target_sha: &str,
    head_sha: &str,
    summary: &ReviewSummary,
) -> Result<Option<Vec<String>>> {
    let (repo_config, _etag) = target.remote.get_repo_config().await?;
    let mut required: BTreeSet<String> = repo_config.required_reviewers.clone();

    if let Ok(bytes) = git::browse::read_blob(git_dir, target_sha, CODEOWNERS_PATH).await {
        let content = String::from_utf8_lossy(&bytes);
        let rules = git::codeowners::parse(&content);
        for path in git::browse::diff_range_name_only(git_dir, target_sha, head_sha).await? {
            required.extend(git::codeowners::owners_for_path(&rules, &path).iter().cloned());
        }
    }

    required.remove(&pr.author);
    let approved: std::collections::BTreeSet<&str> = summary.approved_by.iter().map(String::as_str).collect();
    let missing: Vec<String> = required
        .into_iter()
        .filter(|r| !approved.contains(r.as_str()))
        .collect();
    Ok((!missing.is_empty()).then_some(missing))
}

pub async fn merge(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    merged_by: &str,
    strategy: MergeStrategy,
    locale: gitquarry_i18n::Locale,
) -> Result<MergeOutcome> {
    let backend = ctx.backend();
    let (pr, pr_etag) = pull_requests::get_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, number).await?;
    let Some(mut pr) = pr else {
        bail!("{}", gitquarry_i18n::shared::pull_request_not_found(locale, number));
    };
    if pr.status != PrStatus::Open {
        bail!("{}", gitquarry_i18n::shared::pull_request_wrong_status(locale));
    }
    if pr.is_draft {
        bail!("{}", gitquarry_i18n::shared::pull_request_is_draft(locale));
    }
    let reviews = list_reviews(ctx, target, number).await?;
    let summary = review_summary(&reviews);
    if !summary.changes_requested_by.is_empty() {
        bail!(
            "{}",
            gitquarry_i18n::shared::pull_request_changes_requested_pending(locale)
        );
    }

    let mirror = ctx.open_mirror(target).await?;
    let git_dir = &mirror.path;
    let head_ref = fetch_source_ref(ctx, target, git_dir, &pr).await?;

    let (manifest, manifest_etag) = target.remote.get_manifest().await?;
    let target_ref = format!("refs/heads/{}", pr.target_branch);
    let Some(target_sha) = manifest.refs.get(&target_ref).cloned() else {
        bail!("{}", gitquarry_i18n::shared::not_a_branch(locale, &pr.target_branch));
    };
    let Some(head_sha) = git::browse::resolve_commit(git_dir, &head_ref).await? else {
        bail!("{}", gitquarry_i18n::shared::not_a_branch(locale, &pr.source_branch));
    };

    if let Some(missing) = missing_required_reviewers(target, git_dir, &pr, &target_sha, &head_sha, &summary).await? {
        bail!(
            "{}",
            gitquarry_i18n::shared::pull_request_missing_required_reviewers(locale, &missing.join(", "))
        );
    }

    let (merger_name, merger_email) = merger_identity(ctx, merged_by).await?;
    let merge_sha = match merge_commit(
        git_dir,
        &pr,
        &target_sha,
        &head_sha,
        strategy,
        (&merger_name, &merger_email),
    )
    .await?
    {
        MergeOutcome::Conflict(paths) => return Ok(MergeOutcome::Conflict(paths)),
        MergeOutcome::Merged(sha) => sha,
    };

    let scratch = git::scratch_dir(git_dir)?;
    if let Some(pack) = git::pack_objects_for_push(git_dir, &merge_sha, &[target_sha], scratch.path()).await? {
        target
            .remote
            .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
            .await?;
    }

    let mut new_manifest = manifest;
    new_manifest.refs.insert(target_ref, merge_sha.clone());
    let cas = target
        .remote
        .put_manifest(&new_manifest, manifest_etag.as_deref())
        .await?;
    if cas != Cas::Ok {
        bail!("{}", gitquarry_i18n::shared::cas_conflict(locale));
    }
    // Fired here, not after the metadata `put_pull_request` below: the
    // merge commit landing on the target branch is what "merged" means,
    // and that CAS above just confirmed it — a concurrent-edit conflict on
    // the PR's own metadata afterward doesn't unmerge it (see that put's
    // own comment).
    crate::events::emit(
        ctx,
        crate::events::RepoEvent::PullRequest {
            repo_uid: target.uid.clone(),
            number,
            action: crate::events::PullRequestAction::Merged,
        },
    );
    let stamp = now_stamp();
    pr.status = PrStatus::Merged;
    pr.merge_commit_sha = Some(merge_sha.clone());
    pr.merged_by = Some(merged_by.to_string());
    pr.merged_at = Some(stamp.clone());
    pr.updated_at = stamp;
    // The merge itself already landed on the target branch at this point —
    // a CAS conflict here (someone edited the PR concurrently) must not
    // roll that back, so it's a warning, not a bail: the merge commit stays
    // the source of truth regardless of whether this metadata update wins.
    if pull_requests::put_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, &pr, pr_etag.as_deref()).await?
        == Cas::Conflict
    {
        eprintln!(
            "gitquarry: warning: pull request #{number} merged but its metadata CAS conflicted (concurrent edit) — status may not show as merged"
        );
    }

    Ok(MergeOutcome::Merged(merge_sha))
}

/// Records a PR as merged without touching git at all — `mirror::external`'s
/// Track B PR sync calls this when it observes upstream (GitHub/GitLab/
/// Codeberg) already reports a linked PR as merged: that merge already
/// happened on the provider's side, and its merge commit arrives locally
/// through Track A's own pull-mirror sync, not through [`merge`]'s real
/// `merge-tree`/`commit-tree` path — calling that here would create a
/// second, redundant local merge commit alongside the one already inbound.
/// Idempotent: already-`Merged` is a no-op success, not an error, since a
/// later sync tick re-observing the same upstream merge is the expected
/// steady state, not a bug.
pub async fn mark_merged_externally(
    ctx: &AppContext,
    target: &ResolvedRepo,
    number: u64,
    merge_commit_sha: &str,
    merged_by: Option<&str>,
) -> Result<()> {
    let backend = ctx.backend();
    let (pr, etag) = pull_requests::get_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, number).await?;
    let Some(mut pr) = pr else {
        bail!("no such pull request: #{number}");
    };
    if pr.status == PrStatus::Merged {
        return Ok(());
    }
    let stamp = now_stamp();
    pr.status = PrStatus::Merged;
    pr.merge_commit_sha = Some(merge_commit_sha.to_string());
    pr.merged_by = merged_by.map(str::to_string);
    pr.merged_at = Some(stamp.clone());
    pr.updated_at = stamp;
    match pull_requests::put_pull_request(backend.as_ref(), &ctx.cipher, &target.uid, &pr, etag.as_deref()).await? {
        Cas::Ok => Ok(()),
        Cas::Conflict => bail!("concurrent edit while marking pull request #{number} merged externally"),
    }
}
