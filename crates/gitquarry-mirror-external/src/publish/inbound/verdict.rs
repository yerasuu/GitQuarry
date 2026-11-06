//! What a dry run answers: a translation, or the one rule that refused the
//! pull request.

use super::PrNumber;

/// Which cap a pull request went over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    Commits,
    ChangedFiles,
    BlobBytes,
    PrBytes,
    FetchBytes,
    FetchTime,
    TreeDepth,
}

/// Why a pull request doesn't translate: a closed set, so a report can group
/// by it (§3.14). Worded for the operator, never with a filter pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reason {
    /// The public base is no commit this mirror published in the epoch on
    /// the public side, nor in any earlier one: somebody else pushed it
    /// (or force-pushed the public branch), or the pull request has no
    /// history in common with the public branch.
    NoPrivateBase,
    /// More than one private commit produced the public base on its own, or
    /// the head and the branch have more than one merge base.
    AmbiguousBase,
    /// The base was published under this earlier epoch: the pull request
    /// has to be rebased onto the current public history.
    OldBaseEpoch(u32),
    /// The mirror's filter changed since the public side was published (an
    /// unpublished epoch bump, or a changed `.gitquarry/publish.toml`), so
    /// no translation can be checked against what the next publish does.
    FilterChanged,
    /// The head is the public base itself.
    NoChanges,
    MergeCommit,
    UnsafePath,
    /// A path the publish filter keeps private.
    ExcludedPath,
    /// `.gitattributes`, `.gitmodules`, CI definitions, `.gitquarry/`.
    SensitivePath,
    Symlink,
    Gitlink,
    /// A tree entry mode other than a regular or an executable file.
    UnsupportedMode,
    /// Two names in one directory that differ only by ASCII case.
    CaseCollision,
    /// The public side's old version of a path is not what the private tree
    /// holds there (§3.3 step 2.4).
    PreconditionFailed,
    /// A blob the publish only ever saw at an excluded path (§3.13).
    PrivateBlob,
    ControlChars,
    TooLarge(Cap),
    /// The translation doesn't publish back to the same public commit.
    RoundTripMismatch,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPrivateBase => f.write_str("the base is not a commit this mirror published"),
            Self::AmbiguousBase => f.write_str("the base resolves to more than one private commit"),
            Self::OldBaseEpoch(epoch) => write!(f, "the base was published under the earlier epoch {epoch}"),
            Self::FilterChanged => f.write_str("the publish filter changed since the public side was published"),
            Self::NoChanges => f.write_str("the pull request has no commit on top of its base"),
            Self::MergeCommit => f.write_str("a merge commit: rebase the pull request onto its base branch"),
            Self::UnsafePath => f.write_str("a path that is unsafe on some filesystem"),
            Self::ExcludedPath => f.write_str("a path this repository's publish filter does not accept"),
            Self::SensitivePath => f.write_str("a path no inbound change may touch"),
            Self::Symlink => f.write_str("a symbolic link"),
            Self::Gitlink => f.write_str("a submodule"),
            Self::UnsupportedMode => f.write_str("an entry that is not a regular file"),
            Self::CaseCollision => f.write_str("two names that differ only by case"),
            Self::PreconditionFailed => f.write_str("the old version of a path differs from the private one"),
            Self::PrivateBlob => f.write_str("content the publish only saw at a private path"),
            Self::ControlChars => f.write_str("control characters in a commit message or identity"),
            Self::TooLarge(cap) => write!(f, "over the {cap} cap"),
            Self::RoundTripMismatch => f.write_str("the translation does not publish back to the same commit"),
        }
    }
}

impl std::fmt::Display for Cap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Commits => "commit count",
            Self::ChangedFiles => "changed-file count",
            Self::BlobBytes => "blob size",
            Self::PrBytes => "pull request size",
            Self::FetchBytes => "fetch size",
            Self::FetchTime => "fetch time",
            Self::TreeDepth => "tree depth",
        })
    }
}

/// A refusal and where it applies: the public commit and the path, when the
/// rule is about one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: Reason,
    pub public_commit: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslatedCommit {
    pub public: String,
    /// Computed, never written anywhere.
    pub private: String,
}

/// What the secret scan of the outbound audit finds in what the pull
/// request adds (§3.12, "Secrets in the PR"). Informational: the content is
/// already public, so a finding never refuses the pull request; it tells
/// the maintainer what the private repo would start to store. Names the
/// rule and the line, never the matched text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretFinding {
    /// In a blob, named by the first path the pull request adds it at.
    Blob {
        path: String,
        rule: &'static str,
        line: usize,
    },
    Message {
        public_commit: String,
        rule: &'static str,
        line: usize,
    },
    /// A text blob past the scanner's size limit: unscanned, not clean.
    Unscanned { path: String },
}

#[derive(Debug)]
pub struct Translation {
    /// The private commit that produced the public base (§3.2, `C_o`).
    pub private_base: String,
    /// Later private commits that collapsed onto the same public base. They
    /// are not lost: they are the other side when the merge request merges.
    pub collapsed_onto_base: usize,
    /// Oldest first, one per public commit of the pull request.
    pub commits: Vec<TranslatedCommit>,
    pub secrets: Vec<SecretFinding>,
}

#[derive(Debug)]
pub enum Verdict {
    Translatable(Translation),
    Refused(Refusal),
}

#[derive(Debug)]
pub struct DryRun {
    pub pr: PrNumber,
    /// The public epoch the base was looked up in.
    pub epoch: u32,
    /// `None` when the dry run stopped before the fetch finished.
    pub head: Option<String>,
    /// The merge base of the head and the public branch, when there is one.
    pub base: Option<String>,
    pub verdict: Verdict,
}
