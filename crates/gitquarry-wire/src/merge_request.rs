use crate::uid::Uid;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum MrStatus {
    #[default]
    Open,
    Closed,
    Merged,
}

/// How an MR's commits land on its target branch. `Squash`/`Rebase` both
/// still run the same conflict check ([`Squash`] via a normal 3-way merge
/// tree, [`Rebase`] per source commit) — a conflict aborts the same way
/// regardless of strategy, nothing is written until the whole thing is
/// clean.
///
/// [`Squash`]: MergeStrategy::Squash
/// [`Rebase`]: MergeStrategy::Rebase
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum MergeStrategy {
    #[default]
    Merge,
    Squash,
    Rebase,
    /// Move the target branch to the source's tip, creating nothing — the
    /// only strategy that produces no new commit at all, so what lands is
    /// the author's own commits with the author's own signatures, since
    /// every other strategy has the server build a commit object and a
    /// server can only sign with a key of its own, never the author's
    /// private half. Only possible while the target branch is an ancestor
    /// of the source; once they diverge there is no pointer move that
    /// expresses the merge, so one of the other three has to build a commit.
    FastForward,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MergeRequestMeta {
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub title: String,
    /// Markdown, rendered the same way a release's notes / a repo's README
    /// are.
    #[serde(default)]
    pub description: String,
    /// Equal to the owning repo's own uid for a same-repo MR; a different
    /// uid (a fork's) for a cross-fork one.
    #[serde(default)]
    pub source_repo: Uid,
    #[serde(default)]
    pub source_branch: String,
    #[serde(default)]
    pub target_branch: String,
    #[serde(default)]
    pub status: MrStatus,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub merged_by: Option<String>,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
    /// A draft MR can't be merged (`domain::merge_request::merge` in
    /// `gitquarry-core` bails) until `domain::merge_request::mark_ready`
    /// flips this back — same "opened but not ready to land" signal
    /// GitHub's own draft MRs give, scoped down to just the merge-blocking
    /// bit, no CI-status integration.
    #[serde(default)]
    pub is_draft: bool,
    /// Label names, same free-text-then-validate-against-`RepoConfig.labels`
    /// convention an issue's own labels field already uses — same repo-wide
    /// vocabulary, shared between issues and MRs rather than each having
    /// its own.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Usernames, validated against `access::get_user` at edit time, same
    /// free-text-then-validate convention every other username field in
    /// this codebase follows.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Usernames asked to review this MR — distinct from [`MrReview`],
    /// which is a review someone actually submitted. Purely a to-do-list
    /// signal: nothing here gates who's *allowed* to submit a review.
    #[serde(default)]
    pub requested_reviewers: Vec<String>,
}

/// Which side of a diff a [`CommentAnchor`]'s line number counts on: a
/// removed line only exists on the old side, an added one only on the new.
/// A context line exists on both — those anchor to [`DiffSide::New`], so a
/// comment stays put as long as the line survives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum DiffSide {
    Old,
    #[default]
    New,
}

/// Where in a diff a comment hangs. Deliberately positional (path + side +
/// line), not content-addressed: it points at the diff as rendered for the
/// MR's *current* head, so a comment whose line no longer appears in that
/// diff falls back to the flat thread rather than pretending to still mark
/// a spot that moved.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CommentAnchor {
    /// Repo-relative path as the diff names it, no `a/`/`b/` prefix.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub side: DiffSide,
    /// 1-based line number within `path` on `side`.
    #[serde(default)]
    pub line: u32,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MrComment {
    #[serde(default)]
    pub author: String,
    /// Markdown, rendered the same way a release's notes / a repo's README
    /// are.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    /// `None` for a genuinely local comment; `Some(mirror_id)` for one
    /// pulled in from that external mirror — `docs/external-mirror-concept.md`
    /// Track B's anti-loop bookkeeping: the export filter ("which local
    /// comments are mine to push out") is exactly `origin.is_none()`, so an
    /// imported comment never gets echoed back to where it came from.
    /// `#[serde(default)]` so every pre-existing stored comment
    /// deserializes as `None`, same "genuinely local" meaning it always
    /// had.
    #[serde(default)]
    pub origin: Option<String>,
    /// `None` for a plain thread comment; `Some(_)` for one left on a
    /// specific diff line. `#[serde(default)]` so every comment stored
    /// before line comments existed (and every federation peer, which may
    /// not send this) reads back as a plain thread comment.
    #[serde(default)]
    pub anchor: Option<CommentAnchor>,
}

/// A reviewer's verdict on an MR. `Comment` is the neutral one — a review
/// body without a position either way, the same escape hatch GitHub's own
/// review form offers when a reviewer wants to weigh in without blocking or
/// blessing the change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum ReviewVerdict {
    Approved,
    ChangesRequested,
    #[default]
    Commented,
}

/// One submitted review. Distinct from a [`MrComment`]: a comment is just
/// thread chatter, a review carries a [`ReviewVerdict`] that counts toward
/// the MR's review state. A reviewer can submit several over an MR's life —
/// only their latest one counts (see `domain::merge_request::review_summary`).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MrReview {
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub verdict: ReviewVerdict,
    /// Markdown, rendered the same way a [`MrComment`]'s body is. May be
    /// empty: a bare approval with nothing to say is a normal review.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
}

/// One MR plus its comments, bundled — the wire shape both core's own
/// `/_api/pulls/detail` and (after the binary split) federation's
/// `/_internal/merge-requests` proxy serialize, since both sides of the
/// same cross-instance call need the identical shape.
#[derive(Debug, Serialize, Deserialize)]
pub struct MrDetail {
    #[serde(default)]
    pub mr: MergeRequestMeta,
    #[serde(default)]
    pub comments: Vec<MrComment>,
    /// `#[serde(default)]` so a peer running a build from before reviews
    /// existed still deserializes — it just reports none.
    #[serde(default)]
    pub reviews: Vec<MrReview>,
}
