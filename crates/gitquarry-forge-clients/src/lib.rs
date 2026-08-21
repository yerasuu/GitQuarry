//! `ForgeApi`: one shared trait, one thin adapter per external forge
//! (GitHub/GitLab/Codeberg) — Track B of `docs/external-mirror-concept.md`.
//! Same "one shared shape, several thin adapters" pattern
//! `storage::merge_requests::MrDetail` already established for core's
//! own MR-proxy: the sync engine (`mirror::external`'s MR-sync pass, still
//! to land) only ever talks to `dyn ForgeApi`, never to a specific
//! provider's REST shape directly.
//!
//! GitHub was built and validated against a real account first, its own
//! trait shape and anti-loop bookkeeping (`mirror::external::mr_sync`)
//! proven before GitLab/Codeberg followed as thin impls of the same trait
//! (CLAUDE.md's no-speculative-abstraction rule: validate against one real
//! target before multiplying it by three).

pub mod codeberg;
pub mod github;
pub mod gitlab;

use anyhow::{Result, bail};

/// Public host of every forge this crate has a real client for — the set an
/// instance's external-mirror allowlist starts with
/// (`ServerSettings::mirror_allowed_hosts`, resolved by
/// `gitquarry_mirror_external::validate::effective_allowed_hosts`).
///
/// Lives here rather than in the settings or the sync engine because it's a
/// fact about *which providers are implemented*: a fourth adapter lands
/// with its host in this array, and nothing else has to be told. Only the
/// public instances — a self-hosted GitLab or Gitea is somebody's own host,
/// which only that operator can name.
///
/// Safe as a default in a way an arbitrary host isn't: these three are the
/// mirroring feature's whole point, and none of them is on anybody's
/// internal network, which is what the allowlist exists to keep the server
/// away from.
pub const PUBLIC_FORGE_HOSTS: [&str; 3] = ["github.com", "gitlab.com", "codeberg.org"];

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
/// flatter than `MergeRequestMeta` (no local `source_repo` uid, no
/// `description` markdown-vs-plain distinction): this is only ever compared
/// against `ExternalPrLink`/mapped into a local `MergeRequestMeta`, never
/// stored verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteMr {
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
    pub state: RemoteMrState,
    pub author: String,
    pub created_at: String,
    pub updated_at: String,
    pub merged_at: Option<String>,
    pub merged_by: Option<String>,
    pub merge_commit_sha: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RemoteMrState {
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
pub struct NewRemoteMr {
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
pub struct MrPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<RemoteMrState>,
}

/// One external forge's MR/MR + comment API, reduced to exactly what
/// `mirror::external`'s MR-sync pass needs — never a general-purpose
/// provider SDK. `Send + Sync` so a `Box<dyn ForgeApi>` can cross an
/// `.await` inside the sync loop's own concurrency (same bound
/// `s3db::Backend` already carries for the same reason).
#[async_trait::async_trait]
pub trait ForgeApi: Send + Sync {
    /// Every MR updated at-or-after `since` (an RFC 3339 timestamp, the
    /// provider's own `updated_at`), or every MR at all if `since` is
    /// `None` — a mirror's very first MR-sync tick has no cursor yet. Newest
    /// first, so `since`-less callers can find the first entry outside
    /// their window without necessarily paging through everything (a given
    /// impl remains free to just fetch one page if that's already enough).
    async fn list_merge_requests(&self, since: Option<&str>) -> Result<Vec<RemoteMr>>;
    async fn get_merge_request(&self, number: u64) -> Result<Option<RemoteMr>>;
    async fn create_merge_request(&self, new: NewRemoteMr) -> Result<RemoteMr>;
    async fn update_merge_request(&self, number: u64, patch: MrPatch) -> Result<()>;
    /// `Ok(RemoteMr)` with `state: Merged` and `merge_commit_sha` set on
    /// success — the sync engine needs that sha immediately to call
    /// `domain::merge_request::mark_merged_externally`, not on a follow-up
    /// `get_merge_request`.
    async fn accept_merge_request(&self, number: u64) -> Result<RemoteMr>;
    async fn list_comments(&self, number: u64) -> Result<Vec<RemoteComment>>;
    /// Returns the newly created comment (with its provider-assigned `id`)
    /// so the caller can seed `synced_remote_comment_ids` with it
    /// immediately — see [`RemoteComment::id`]'s own doc comment.
    async fn add_comment(&self, number: u64, body: &str) -> Result<RemoteComment>;
}
