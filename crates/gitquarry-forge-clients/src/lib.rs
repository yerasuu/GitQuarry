//! `ForgeApi`: one shared trait, one thin adapter per external forge
//! (GitHub/GitLab/Codeberg) — Track B of `docs/external-mirror-concept.md`.
//! Same "one shared shape, several thin adapters" pattern
//! `storage::pull_requests::PrDetail` already established for core's
//! own PR-proxy: the sync engine (`mirror::external`'s PR-sync pass, still
//! to land) only ever talks to `dyn ForgeApi`, never to a specific
//! provider's REST shape directly.
//!
//! GitHub was built and validated against a real account first, its own
//! trait shape and anti-loop bookkeeping (`mirror::external::pr_sync`)
//! proven before GitLab/Codeberg followed as thin impls of the same trait
//! (CLAUDE.md's no-speculative-abstraction rule: validate against one real
//! target before multiplying it by three).

pub mod codeberg;
pub mod github;
pub mod gitlab;

use anyhow::{Result, bail};

/// Every adapter here splices a repo's owner/name straight into a REST
/// path, unescaped. Forge account and repository names are already limited
/// to this alphabet on all three providers, so anything outside it can't
/// name a real repo — but it *can* smuggle a `?`, `#` or `/` into the URL
/// and turn the request into one against a different endpoint. Rejected at
/// construction instead of escaped at every use site, same call
/// `mirror-external`'s `validate_mirror_token` already makes for the token.
pub(crate) fn check_path_segment(kind: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("invalid {kind} in mirror url: {value:?}");
    }
    // `.` and `..` pass the alphabet above (a dot is legal inside a real
    // name) but name a path traversal rather than a repo — and the HTTP
    // client resolves them before the request goes out, silently
    // retargeting it at a different endpoint.
    if value == "." || value == ".." {
        bail!("invalid {kind} in mirror url: {value:?}");
    }
    Ok(())
}

/// A pull/merge request as the external side represents it — deliberately
/// flatter than `PullRequestMeta` (no local `source_repo` uid, no
/// `description` markdown-vs-plain distinction): this is only ever compared
/// against `ExternalPrLink`/mapped into a local `PullRequestMeta`, never
/// stored verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemotePr {
    pub number: u64,
    pub title: String,
    pub body: String,
    /// Branch name only, no remote/owner prefix (GitHub's `head.ref`,
    /// GitLab's `source_branch`, Gitea's `head.ref`) — this repo's own
    /// mirrored copy of that branch (Track A already keeps it in sync) is
    /// what `source_branch`/`target_branch` resolve against locally, so a
    /// fork-qualified ref would be meaningless here.
    pub source_branch: String,
    pub target_branch: String,
    pub state: RemotePrState,
    pub author: String,
    pub created_at: String,
    pub updated_at: String,
    pub merged_at: Option<String>,
    pub merged_by: Option<String>,
    pub merge_commit_sha: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RemotePrState {
    #[default]
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteComment {
    /// Provider-native comment id — `ExternalPrLink::synced_remote_comment_ids`'s
    /// element type, the anti-loop inbound-dedup key
    /// (`docs/external-mirror-concept.md`'s "Comentarios hacia adentro").
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewRemotePr {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
}

/// Only the fields a title/description/state edit can touch — `None` means
/// "leave as is", same convention `mirror::external::domain::update`'s
/// caller uses for an unrotated token, so a caller only ever has to name
/// what actually changed.
#[derive(Debug, Clone, Default)]
pub struct PrPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<RemotePrState>,
}

/// One external forge's PR/MR + comment API, reduced to exactly what
/// `mirror::external`'s PR-sync pass needs — never a general-purpose
/// provider SDK. `Send + Sync` so a `Box<dyn ForgeApi>` can cross an
/// `.await` inside the sync loop's own concurrency (same bound
/// `s3db::Backend` already carries for the same reason).
#[async_trait::async_trait]
pub trait ForgeApi: Send + Sync {
    /// Every PR updated at-or-after `since` (an RFC 3339 timestamp, the
    /// provider's own `updated_at`), or every PR at all if `since` is
    /// `None` — a mirror's very first PR-sync tick has no cursor yet. Newest
    /// first, so `since`-less callers can find the first entry outside
    /// their window without necessarily paging through everything (a given
    /// impl remains free to just fetch one page if that's already enough).
    async fn list_pull_requests(&self, since: Option<&str>) -> Result<Vec<RemotePr>>;
    async fn get_pull_request(&self, number: u64) -> Result<Option<RemotePr>>;
    async fn create_pull_request(&self, new: NewRemotePr) -> Result<RemotePr>;
    async fn update_pull_request(&self, number: u64, patch: PrPatch) -> Result<()>;
    /// `Ok(RemotePr)` with `state: Merged` and `merge_commit_sha` set on
    /// success — the sync engine needs that sha immediately to call
    /// `domain::pull_request::mark_merged_externally`, not on a follow-up
    /// `get_pull_request`.
    async fn merge_pull_request(&self, number: u64) -> Result<RemotePr>;
    async fn list_comments(&self, number: u64) -> Result<Vec<RemoteComment>>;
    /// Returns the newly created comment (with its provider-assigned `id`)
    /// so the caller can seed `synced_remote_comment_ids` with it
    /// immediately — see [`RemoteComment::id`]'s own doc comment.
    async fn add_comment(&self, number: u64, body: &str) -> Result<RemoteComment>;
}
