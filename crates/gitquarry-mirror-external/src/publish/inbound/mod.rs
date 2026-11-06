//! Inbound, phase 0 (`docs/publish-mirror-inbound-concept.md` §7): whether
//! one public pull request of a history publish mirror translates into
//! private commits, and if not, which rule refuses it. A dry run: it decides
//! and reports, and nothing lands anywhere.
//!
//! The pull request is fetched from the mirror's own public remote into a
//! quarantine bare repo in a temporary directory, removed when the dry run
//! ends; never into the private repo's object store, and never to the
//! bucket. Its base is resolved through the epoch's inverse index
//! ([`super::inverse`]), each public commit is overlaid on the private tree
//! of the commit before it (§3.3), and the forward rewrite is run over the
//! result to prove it gives back the very public commits of the pull request
//! (`rewrite(X') == X`). The translated private commits exist only in
//! memory, as objects the private repo's reader answers without them being
//! written ([`super::objects::ObjectReader::insert`]).
//!
//! Every input from the public side is hostile: the pull request's objects,
//! its paths and its messages. They are capped before they are worked on
//! ([`MAX_PR_COMMITS`] and the other `MAX_*`), fsck'd as they arrive, and
//! refused rather than truncated or guessed at.

pub mod command;
mod quarantine;
mod translate;

use super::epoch::published_epoch;
use super::log::PublishLog;
use super::mapping::{Table, epoch_prefix, from_map_value};
use super::objects::{Oid, to_hex};
use super::preview::is_published_ref;
use super::scratch::init_scratch;
use super::{MirrorToken, PublishMirror, PublishMode};
use crate::access::ExternalMirrorProvider;
use crate::git_remote;
use crate::validate::{
    check_host_allowed, effective_allowed_hosts, validate_branch_name, validate_mirror_id, validate_pr_number,
};
use anyhow::{Context, Result, bail};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::uid::Uid;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The caps of §3.12, fixed per instance (Q15 of the owner's answers, kb
/// #117). Chosen to sit at or above what the big forges themselves show in
/// one pull request (GitHub lists at most 250 commits and 3000 files), not
/// measured on real traffic yet; a pull request over any of them is refused
/// whole, never translated in part.
pub const MAX_PR_COMMITS: usize = 250;
pub const MAX_CHANGED_FILES: usize = 3_000;
/// Largest blob a pull request may add, read from `cat-file --batch-check`
/// before anything else looks at it.
pub const MAX_BLOB_BYTES: u64 = 16 * 1024 * 1024;
/// Every blob the pull request adds, together.
pub const MAX_PR_BYTES: u64 = 64 * 1024 * 1024;
/// The quarantine repo after the fetch: the public history the pull request
/// sits on comes along with it, so this bounds that too.
pub const MAX_QUARANTINE_BYTES: u64 = 1024 * 1024 * 1024;
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// A public pull request's number, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrNumber(u64);

impl PrNumber {
    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::str::FromStr for PrNumber {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        validate_pr_number(raw).map(Self)
    }
}

impl std::fmt::Display for PrNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A short branch name of the public remote, validated: it is sent to that
/// remote inside a refspec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBranch(String);

impl PublicBranch {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for PublicBranch {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        validate_branch_name(raw)?;
        Ok(Self(raw.to_string()))
    }
}

/// Which pull request to look at, and on which public branch it is opened.
/// Phase 0 has no forge API, so the base branch is the operator's word.
#[derive(Debug)]
pub struct Request {
    pub pr: PrNumber,
    pub base: PublicBranch,
    /// Decides the ref the pull request's head is fetched from.
    pub provider: ExternalMirrorProvider,
}

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

#[derive(Debug)]
pub struct Translation {
    /// The private commit that produced the public base (§3.2, `C_o`).
    pub private_base: String,
    /// Later private commits that collapsed onto the same public base. They
    /// are not lost: they are the other side when the merge request merges.
    pub collapsed_onto_base: usize,
    /// Oldest first, one per public commit of the pull request.
    pub commits: Vec<TranslatedCommit>,
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

/// Looks at pull request `request.pr` of the public remote of `mirror`, a
/// history publish mirror of repo `uid` whose private repo is the bare repo
/// at `private_repo`, and says whether it translates.
///
/// Writes nothing to the bucket and nothing to `private_repo`; the only
/// thing written is the quarantine repo, in a temporary directory removed
/// before this returns. `token` reaches git out of band, never in argv or
/// in an error.
///
/// # Errors
///
/// What keeps the question from being answered: a snapshot mirror, a base
/// branch the mirror doesn't publish, a provider with no pull request refs,
/// a host the instance doesn't allow, a mirror that never published, a
/// failed fetch (no such pull request, the remote down), a corrupt bucket
/// object, an unreadable private repo. A pull request that can't be
/// translated is not an error: it is [`Verdict::Refused`].
pub async fn dry_run(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    private_repo: &Path,
    mirror: &PublishMirror,
    request: &Request,
    token: &MirrorToken,
) -> Result<DryRun> {
    if mirror.mode != PublishMode::History {
        bail!("inbound needs a history publish mirror: a snapshot has no public commit that maps back");
    }
    validate_mirror_id(&mirror.id)?;
    let base_ref = format!("refs/heads/{}", request.base.as_str());
    if !is_published_ref(mirror, &base_ref) {
        bail!("{} is not a branch this mirror publishes", request.base.as_str());
    }
    let pull_ref = pull_ref(request.provider, request.pr)?;
    let (settings, _etag) = gitquarry_core::storage::settings::get_settings(backend, cipher).await?;
    check_host_allowed(&mirror.remote_url, &effective_allowed_hosts(&settings))?;
    let log = super::log::read(backend, cipher, uid, &mirror.id).await?;
    let Some(epoch) = published_epoch(&log) else {
        bail!("this mirror has published nothing yet, so no public commit has a private counterpart");
    };

    let scratch = gitquarry_core::git::scratch_dir(&std::env::temp_dir())?;
    let job = Job {
        backend,
        cipher,
        uid,
        private_repo: private_repo.to_path_buf(),
        quarantine: scratch.path().join("quarantine.git"),
        mirror,
        log: &log,
        epoch,
    };
    let mut seen = Seen::default();
    let verdict = match job.run(&pull_ref, &base_ref, token, &mut seen).await {
        Ok(translation) => Verdict::Translatable(translation),
        Err(Stop::Refused(refusal)) => Verdict::Refused(refusal),
        Err(Stop::Failed(error)) => return Err(error),
    };
    drop(scratch);
    Ok(DryRun {
        pr: request.pr,
        epoch,
        head: seen.head.as_ref().map(to_hex),
        base: seen.base.as_ref().map(to_hex),
        verdict,
    })
}

/// Where the forge keeps a pull request's head:
/// [GitHub](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests/checking-out-pull-requests-locally)
/// under `refs/pull/<n>/head`, which Gitea and Forgejo (Codeberg) mirror;
/// [GitLab](https://docs.gitlab.com/user/project/merge_requests/merge_request_troubleshooting/#check-out-merge-requests-locally-through-the-head-ref)
/// under `refs/merge-requests/<n>/head`. Fetched from the mirror's own
/// remote, never from the fork's URL, so a fork never sees the token (§4).
fn pull_ref(provider: ExternalMirrorProvider, pr: PrNumber) -> Result<String> {
    Ok(match provider {
        ExternalMirrorProvider::GitHub | ExternalMirrorProvider::Codeberg => format!("refs/pull/{pr}/head"),
        ExternalMirrorProvider::GitLab => format!("refs/merge-requests/{pr}/head"),
        ExternalMirrorProvider::Other => bail!("no known pull request refs for this forge: name the provider"),
    })
}

/// What the report can say about the public side, as far as the dry run got.
#[derive(Default)]
struct Seen {
    head: Option<Oid>,
    base: Option<Oid>,
}

/// Why the dry run stopped before a translation: a verdict on the pull
/// request, or a failure to reach one.
enum Stop {
    Refused(Refusal),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for Stop {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

fn refuse(reason: Reason, public_commit: Option<&Oid>, path: Option<&str>) -> Stop {
    Stop::Refused(Refusal {
        reason,
        public_commit: public_commit.map(to_hex),
        path: path.map(str::to_string),
    })
}

struct Job<'a> {
    backend: &'a dyn Backend,
    cipher: &'a Cipher,
    uid: &'a Uid,
    private_repo: PathBuf,
    quarantine: PathBuf,
    mirror: &'a PublishMirror,
    log: &'a PublishLog,
    epoch: u32,
}

impl Job<'_> {
    async fn run(
        &self,
        pull_ref: &str,
        base_ref: &str,
        token: &MirrorToken,
        seen: &mut Seen,
    ) -> Result<Translation, Stop> {
        init_scratch(&self.quarantine)?;
        let refspecs = [
            format!("+{pull_ref}:{}", quarantine::HEAD_REF),
            format!("+{base_ref}:{}", quarantine::BASE_REF),
        ];
        let fetch = git_remote::fetch_untrusted(&self.quarantine, &self.mirror.remote_url, token.expose(), &refspecs);
        match tokio::time::timeout(FETCH_TIMEOUT, fetch).await {
            Err(_elapsed) => return Err(refuse(Reason::TooLarge(Cap::FetchTime), None, None)),
            Ok(fetched) => fetched.context("fetching the pull request")?,
        }
        if quarantine::bytes_on_disk(&self.quarantine)? > MAX_QUARANTINE_BYTES {
            return Err(refuse(Reason::TooLarge(Cap::FetchBytes), None, None));
        }

        let range = {
            let quarantine = self.quarantine.clone();
            blocking(move || quarantine::public_range(&quarantine)).await??
        };
        seen.head = Some(range.head);
        let base = match range.bases.as_slice() {
            [] => {
                let reason = match self.older_epoch(&range.ancestry).await? {
                    Some(epoch) => Reason::OldBaseEpoch(epoch),
                    None => Reason::NoPrivateBase,
                };
                return Err(refuse(reason, Some(&range.head), None));
            }
            [base] => *base,
            _ => return Err(refuse(Reason::AmbiguousBase, Some(&range.head), None)),
        };
        seen.base = Some(base);
        if range.truncated {
            return Err(refuse(Reason::TooLarge(Cap::Commits), None, None));
        }
        if range.commits.is_empty() {
            return Err(refuse(Reason::NoChanges, Some(&base), None));
        }
        if let Some((merge, _)) = range.commits.iter().find(|(_, parents)| parents.len() != 1) {
            return Err(refuse(Reason::MergeCommit, Some(merge), None));
        }

        let prefix = epoch_prefix(self.uid, &self.mirror.id, self.epoch);
        let candidates = super::inverse::Lookup::new(&prefix)
            .get(self.backend, self.cipher, &base)
            .await?;
        if candidates.is_empty() {
            let reason = match self.older_epoch(&[base]).await? {
                Some(epoch) => Reason::OldBaseEpoch(epoch),
                None => Reason::NoPrivateBase,
            };
            return Err(refuse(reason, Some(&base), None));
        }
        let collapsed_onto_base = candidates.len() - 1;
        let private_base = {
            let repo = self.private_repo.clone();
            blocking(move || translate::originating(&repo, &candidates))
                .await?
                .map_err(|stop| with_commit(stop, &base))?
        };

        let effective = {
            let (repo, mirror) = (self.private_repo.clone(), self.mirror.clone());
            blocking(move || super::history::current_filter(&repo, &mirror)).await??
        };
        if let Some(error) = effective.repo_file_error {
            return Err(Stop::Failed(anyhow::anyhow!(
                "the private repo's publish filter file is invalid ({error}); a publish would refuse it too"
            )));
        }
        if self.mirror.filter_epoch != self.epoch || self.log.fingerprint.as_ref() != Some(&effective.fingerprint) {
            return Err(refuse(Reason::FilterChanged, None, None));
        }

        let translated = {
            let input = translate::Input {
                private_repo: self.private_repo.clone(),
                quarantine: self.quarantine.clone(),
                filter: effective.filter.clone(),
                public_base: base,
                private_base,
                commits: range.commits.iter().map(|(commit, _)| *commit).collect(),
            };
            blocking(move || translate::translate(input)).await??
        };

        let mut excluded = Table::<0>::new(format!("{prefix}/excluded"));
        excluded
            .load(
                self.backend,
                self.cipher,
                translated.added.iter().map(|(blob, _)| blob[0]),
            )
            .await?;
        if let Some((_, path)) = translated.added.iter().find(|(blob, _)| excluded.get(blob).is_some()) {
            return Err(refuse(Reason::PrivateBlob, None, Some(path)));
        }

        let mut map = Table::<40>::new(format!("{prefix}/map"));
        map.load(self.backend, self.cipher, [private_base[0]]).await?;
        let Some((public, public_tree)) = map.get(&private_base).and_then(from_map_value) else {
            return Err(Stop::Failed(anyhow::anyhow!(
                "{} is in the inverse index of epoch {} but not in its mapping",
                to_hex(&private_base),
                self.epoch
            )));
        };
        if public != base {
            return Err(Stop::Failed(anyhow::anyhow!(
                "the mapping and the inverse index of epoch {} disagree on {}",
                self.epoch,
                to_hex(&private_base)
            )));
        }
        let commits = translated.commits.clone();
        {
            let round_trip = translate::RoundTrip {
                reader: translated.reader,
                filter: effective.filter,
                identities: self.mirror.identities.clone(),
                private_base,
                public_base: (public, public_tree),
                commits: translated.commits,
            };
            blocking(move || translate::round_trip(round_trip)).await??;
        }
        Ok(Translation {
            private_base: to_hex(&private_base),
            collapsed_onto_base,
            commits: commits
                .iter()
                .map(|(public, private)| TranslatedCommit {
                    public: to_hex(public),
                    private: to_hex(private),
                })
                .collect(),
        })
    }

    /// The newest earlier epoch whose inverse lists any of `commits`.
    async fn older_epoch(&self, commits: &[Oid]) -> Result<Option<u32>> {
        for epoch in (0..self.epoch).rev() {
            let mut lookup = super::inverse::Lookup::new(&epoch_prefix(self.uid, &self.mirror.id, epoch));
            for commit in commits {
                if !lookup.get(self.backend, self.cipher, commit).await?.is_empty() {
                    return Ok(Some(epoch));
                }
            }
        }
        Ok(None)
    }
}

fn with_commit(stop: Stop, commit: &Oid) -> Stop {
    match stop {
        Stop::Refused(mut refusal) => {
            refusal.public_commit.get_or_insert_with(|| to_hex(commit));
            Stop::Refused(refusal)
        }
        failed @ Stop::Failed(_) => failed,
    }
}

/// The synchronous steps (git pipes, hashing) off the async workers.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .context("the inbound dry-run task panicked")
}
