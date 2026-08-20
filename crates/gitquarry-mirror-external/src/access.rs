//! [`ExternalMirror`] CRUD — one entity per repo-scoped push/pull mirror
//! against a non-gitquarry forge (GitHub/GitLab/Codeberg), see
//! `docs/external-mirror-concept.md`. Hand-built collection, not
//! `#[derive(Entity)]`: same reason as `gitquarry_core::storage::merge_requests`
//! — the path needs the *target* repo's `Uid` at runtime, and
//! `#[entity(collection = "...")]` only accepts a compile-time literal.
//!
//! `token` is stored reversibly (plain string, not a hash): unlike
//! `AccessToken::token_hash`, the sync loop has to reuse the same token on
//! every tick to authenticate against the external forge, so it can't be a
//! one-way hash. `s3db` encrypts every object at rest unconditionally, so
//! this needs no crypto handling of its own beyond that — same shape as
//! an external forge's API credential.

use crate::validate::{validate_mirror_label, validate_mirror_token, validate_mirror_url};
use anyhow::{Result, bail};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{Backend, Cas, REPOS_PREFIX};
use s3db::Collection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many CAS-conflict retries [`update_last_synced`] attempts before
/// giving up — same bound `storage::merge_requests::next_mr_number` uses for
/// its own CAS retry loop.
const UPDATE_MAX_RETRIES: u32 = 8;

/// Longest error text kept on a mirror or one of its refs.
///
/// These strings are whatever the other side said: `git`'s stderr, or an
/// HTTP body a forge returned verbatim (`{url} returned {status}: {body}`
/// in the `gitquarry-forge-clients` adapters). An error page can be
/// megabytes, and this text is stored in the bucket and re-rendered into
/// the settings tab on every load — a diagnostic is worth the first
/// paragraph, never the whole document.
const MAX_ERROR_LEN: usize = 500;

/// Caps `text` at [`MAX_ERROR_LEN`], on a character boundary, marking that
/// it was cut so nobody reads a truncated message as the whole story.
pub fn cap_error(text: &str) -> String {
    if text.len() <= MAX_ERROR_LEN {
        return text.to_string();
    }
    let cut = (0..=MAX_ERROR_LEN)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    format!("{}… (truncated)", &text[..cut])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExternalMirrorProvider {
    #[default]
    GitHub,
    GitLab,
    Codeberg,
    Other,
}

/// Per-ref sync state, UI-facing — kept separate from [`ExternalMirror::last_synced`]
/// so a diverged/errored ref never loses its last known-good sha (see that
/// field's own doc comment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefSyncStatus {
    InSync,
    Diverged,
    Error(String),
}

/// No variant is a "natural" default for a ref never synced before — `InSync`
/// is picked here only as the harmless starting point [`update_ref_status`]'s
/// get/modify/put loop needs before the very first write ever lands.
impl Default for RefSyncStatus {
    fn default() -> Self {
        RefSyncStatus::InSync
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SyncDirection {
    PushOnly,
    PullOnly,
    #[default]
    Bidirectional,
}

impl SyncDirection {
    pub fn allows_push(self) -> bool {
        matches!(self, SyncDirection::PushOnly | SyncDirection::Bidirectional)
    }

    pub fn allows_pull(self) -> bool {
        matches!(self, SyncDirection::PullOnly | SyncDirection::Bidirectional)
    }
}

/// One push/pull mirror linking a gitquarry repo to a ref on an external
/// forge. `remote_url` never carries a token (unlike the credentialed URL
/// built at sync time from `remote_url` + `token`) — same separation as
/// `Peer::git_base_url`/`PeerCredential::secret`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExternalMirror {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub remote_url: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub provider: ExternalMirrorProvider,
    #[serde(default)]
    pub direction: SyncDirection,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub label: String,
    /// ref name -> last sha this mirror successfully synced for it — the
    /// three-way comparison's memory. Never cleared on a diverged/error ref:
    /// see [`RefSyncStatus`] for why that's tracked separately.
    #[serde(default)]
    pub last_synced: BTreeMap<String, String>,
    #[serde(default)]
    pub ref_status: BTreeMap<String, RefSyncStatus>,
    /// Why this mirror's last sync attempt failed as a whole, or `None` if
    /// it got far enough to report per-ref status. Distinct from
    /// [`RefSyncStatus::Error`], which only ever covers one ref: a sync can
    /// also fail before reaching any ref at all (a bad credential rejected
    /// at `ls-remote`, an unreachable host, a local mirror that won't open),
    /// and those used to surface nowhere — the push-triggered sync's stderr
    /// goes to `/dev/null`, so this field is the only place such a failure
    /// is visible.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Track B, opt-in and off by default: bidirectional MR/MR sync against
    /// this same mirror's `remote_url`/`token` — no separate credential.
    /// Every `ExternalMirrorProvider` except `Other` has a real
    /// `forge::ForgeApi` impl (`crate::mr_sync::build_forge`); `Other` is a
    /// UI label for "some git host", not a specific REST API to target, so
    /// turning this on for one fails loudly on the next sync tick rather
    /// than silently doing nothing.
    #[serde(default)]
    pub sync_merge_requests: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

pub fn validate_external_mirror(mirror: &ExternalMirror) -> Result<()> {
    validate_mirror_url(&mirror.remote_url)?;
    validate_mirror_token(&mirror.token)?;
    validate_mirror_label(&mirror.label)?;
    Ok(())
}

/// Random 64-bit hex id — a repo can have more than one external mirror,
/// even against the same provider, so the key can't be derived from
/// provider/url alone. Opaque, not a secret: fine to be short and
/// non-cryptographically-attributed.
pub fn generate_mirror_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

fn mirrors_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/mirrors"))
}

/// Creates (`expected_etag: None`) or updates (`expected_etag: Some(...)`,
/// CAS) one mirror. Two paths, not an upsert like
/// `PeerCredential::set_peer_credential`'s unconditional overwrite: the web
/// UI needs a stable identity per mirror across edits, so an edit must go
/// through the same CAS discipline as `storage::merge_requests::put_merge_request`
/// rather than silently clobbering concurrent changes.
pub async fn set_external_mirror(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror: &ExternalMirror,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    validate_external_mirror(mirror)?;
    mirrors_collection(uid)
        .put(backend, cipher, &mirror.id, mirror, expected_etag)
        .await
}

pub async fn get_external_mirror(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
) -> Result<(Option<ExternalMirror>, Option<String>)> {
    mirrors_collection(uid).get(backend, cipher, mirror_id).await
}

/// Every mirror configured for this repo. Mirror count per repo is
/// operator-curated and expected to stay small, so this fans out a tiny GET
/// per mirror on every call rather than maintaining a cache — same
/// tradeoff a small, operator-managed list makes worthwhile.
pub async fn list_external_mirrors(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<Vec<ExternalMirror>> {
    Ok(list_external_mirrors_with_etags(backend, cipher, uid)
        .await?
        .into_iter()
        .map(|(mirror, _etag)| mirror)
        .collect())
}

/// Same as [`list_external_mirrors`], but keeps each mirror's etag —
/// `webui::repo_settings::mirrors`' edit form needs it as the CAS guard for
/// `edit_mirror`, but nothing on the sync path (`crate::engine::sync_repo_mirrors`)
/// ever writes through this listing, so that one stays on the etag-free
/// variant instead of threading an unused value through its whole call
/// chain.
pub async fn list_external_mirrors_with_etags(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
) -> Result<Vec<(ExternalMirror, String)>> {
    let collection = mirrors_collection(uid);
    let ids = collection.list_names(backend).await?;
    let mut mirrors = Vec::with_capacity(ids.len());
    for id in ids {
        if let (Some(mirror), Some(etag)) = collection.get::<ExternalMirror>(backend, cipher, &id).await? {
            mirrors.push((mirror, etag));
        }
    }
    mirrors.sort_by(|(a, _), (b, _)| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
    Ok(mirrors)
}

pub async fn delete_external_mirror(backend: &dyn Backend, cipher: &Cipher, uid: &Uid, mirror_id: &str) {
    mirrors_collection(uid).delete(backend, cipher, mirror_id).await;
}

/// Links one local `gitquarry_core::storage::merge_requests::MergeRequestMeta`
/// to its counterpart on `mirror_id`'s external forge — Track B, keyed by
/// `local_number` (scoped under one specific mirror, not the whole repo: a
/// repo can have several `ExternalMirror`s, each with independent,
/// unrelated MR sync).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExternalPrLink {
    #[serde(default)]
    pub local_number: u64,
    #[serde(default)]
    pub external_number: u64,
    /// Inbound anti-loop dedup: a remote comment id already imported once
    /// never gets imported again, even if a later `list_comments` still
    /// reports it (`docs/external-mirror-concept.md`'s "Comentarios hacia
    /// adentro").
    #[serde(default)]
    pub synced_remote_comment_ids: std::collections::BTreeSet<u64>,
    /// Outbound anti-loop dedup — **not in the original design doc**: a
    /// local comment's `origin.is_none()` alone only says "this is mine to
    /// export", it never stops the same local comment from being exported
    /// again on the next tick (nothing about exporting it changes `origin`,
    /// nor should it — it's still genuinely local). Keyed by
    /// `storage::merge_requests::list_mr_comments_with_keys`' own per-comment
    /// key, the only stable per-comment identity this codebase has.
    #[serde(default)]
    pub synced_local_comment_keys: std::collections::BTreeSet<String>,
    /// The provider's own MR `updated_at` as of the last successful sync —
    /// both this mirror's `list_merge_requests(since)` cursor for future
    /// ticks, and the last-write-wins baseline for the title/description/
    /// open-closed field sync.
    #[serde(default)]
    pub remote_updated_cursor: String,
}

fn mr_links_collection(uid: &Uid, mirror_id: &str) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/mirrors/{mirror_id}/mr-links"))
}

pub async fn get_mr_link(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    local_number: u64,
) -> Result<(Option<ExternalPrLink>, Option<String>)> {
    mr_links_collection(uid, mirror_id)
        .get(backend, cipher, &local_number.to_string())
        .await
}

pub async fn put_mr_link(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    link: &ExternalPrLink,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    mr_links_collection(uid, mirror_id)
        .put(backend, cipher, &link.local_number.to_string(), link, expected_etag)
        .await
}

/// CAS get/modify/put retry loop over one MR link — the write half of a
/// [`crate::mr_sync`] pass, which accumulates anti-loop bookkeeping
/// (`synced_local_comment_keys`, `synced_remote_comment_ids`, the cursor)
/// as it goes and has to land *all* of it or export the same comments
/// again on the next tick.
///
/// A plain [`put_mr_link`] with the etag read at the start of that pass
/// dropped the whole set on a conflict, silently: the caller unwrapped the
/// `Result` and threw the `Cas` away. Re-reading and re-applying is also
/// the only correct merge here — every field this touches is either a set
/// insertion or a monotonic cursor, so applying it to whatever landed
/// meanwhile loses nothing.
pub async fn merge_mr_link(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    local_number: u64,
    apply: impl Fn(&mut ExternalPrLink),
) -> Result<()> {
    let collection = mr_links_collection(uid, mirror_id);
    let key = local_number.to_string();
    for attempt in 1..=UPDATE_MAX_RETRIES {
        let (link, etag) = collection.get::<ExternalPrLink>(backend, cipher, &key).await?;
        let Some(mut link) = link else {
            bail!("no such MR link: #{local_number} on mirror {mirror_id}");
        };
        apply(&mut link);
        match collection.put(backend, cipher, &key, &link, etag.as_deref()).await? {
            Cas::Ok => return Ok(()),
            Cas::Conflict if attempt < UPDATE_MAX_RETRIES => {
                gitquarry_core::storage::cas_retry_backoff(attempt).await;
                continue;
            }
            Cas::Conflict => break,
        }
    }
    bail!(
        "too many CAS conflicts updating MR link #{local_number} on mirror '{mirror_id}' (giving up after {UPDATE_MAX_RETRIES} attempts)"
    )
}

/// Every MR link under this one mirror. Link count follows MR count on a
/// mirrored repo — small enough in practice for the same "fan out a tiny
/// GET per entry" tradeoff [`list_external_mirrors`] already makes.
pub async fn list_mr_links(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
) -> Result<Vec<ExternalPrLink>> {
    let collection = mr_links_collection(uid, mirror_id);
    let ids = collection.list_names(backend).await?;
    let mut links = Vec::with_capacity(ids.len());
    for id in ids {
        if let (Some(link), _) = collection.get::<ExternalPrLink>(backend, cipher, &id).await? {
            links.push(link);
        }
    }
    Ok(links)
}

/// CAS get/modify/put retry loop that records `sha` as the last-synced sha
/// for `ref_name` and marks it [`RefSyncStatus::InSync`] — its own retry
/// loop (not the caller's) so two refs of the same mirror syncing
/// concurrently (`crate::engine::sync_mirror_once`, one task per ref) never
/// clobber each other's update to the shared `last_synced`/`ref_status`
/// maps.
pub async fn update_last_synced(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    ref_name: &str,
    sha: &str,
) -> Result<()> {
    update_ref_status(backend, cipher, uid, mirror_id, ref_name, |status, last_synced| {
        *status = RefSyncStatus::InSync;
        last_synced.insert(ref_name.to_string(), sha.to_string());
    })
    .await
}

/// Same CAS-retry shape as [`update_last_synced`], but only touches
/// `ref_status` — used when a ref diverges or a sync attempt errors, where
/// `last_synced` must be left exactly as it was (the last known-good sha,
/// never overwritten by a failed or skipped attempt).
pub async fn set_ref_status(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    ref_name: &str,
    status: RefSyncStatus,
) -> Result<()> {
    // Same cap `set_mirror_error` applies one level up, for the same
    // reason: a per-ref error carries the far side's own text too.
    let status = match status {
        RefSyncStatus::Error(msg) => RefSyncStatus::Error(cap_error(&crate::git_remote::redact_url(&msg))),
        other => other,
    };
    update_ref_status(backend, cipher, uid, mirror_id, ref_name, move |s, _| {
        *s = status.clone();
    })
    .await
}

/// Records (`Some`) or clears (`None`) [`ExternalMirror::last_error`] —
/// same CAS-retry shape as [`update_ref_status`], one level up: this is the
/// whole mirror's outcome, not one ref's.
///
/// `error` is passed through [`crate::git_remote::redact_url`] before it's
/// stored. Every git call that takes a credentialed URL already redacts its
/// own `Err` — this is the belt-and-braces pass at the boundary where the
/// text stops being a transient log line and becomes bucket state rendered
/// back into the settings tab.
pub async fn set_mirror_error(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    error: Option<String>,
) -> Result<()> {
    let error = error.map(|e| cap_error(&crate::git_remote::redact_url(&e)));
    let collection = mirrors_collection(uid);
    for attempt in 1..=UPDATE_MAX_RETRIES {
        let (mirror, etag) = collection.get::<ExternalMirror>(backend, cipher, mirror_id).await?;
        let Some(mut mirror) = mirror else {
            bail!("no such external mirror: {mirror_id}");
        };
        if mirror.last_error == error {
            return Ok(());
        }
        mirror.last_error = error.clone();
        match collection
            .put(backend, cipher, mirror_id, &mirror, etag.as_deref())
            .await?
        {
            Cas::Ok => return Ok(()),
            Cas::Conflict if attempt < UPDATE_MAX_RETRIES => {
                gitquarry_core::storage::cas_retry_backoff(attempt).await;
                continue;
            }
            Cas::Conflict => break,
        }
    }
    bail!(
        "too many CAS conflicts updating mirror '{mirror_id}' error state (giving up after {UPDATE_MAX_RETRIES} attempts)"
    )
}

async fn update_ref_status(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    mirror_id: &str,
    ref_name: &str,
    apply: impl Fn(&mut RefSyncStatus, &mut BTreeMap<String, String>),
) -> Result<()> {
    let collection = mirrors_collection(uid);
    for attempt in 1..=UPDATE_MAX_RETRIES {
        let (mirror, etag) = collection.get::<ExternalMirror>(backend, cipher, mirror_id).await?;
        let Some(mut mirror) = mirror else {
            bail!("no such external mirror: {mirror_id}");
        };
        let mut status = mirror.ref_status.get(ref_name).cloned().unwrap_or_default();
        apply(&mut status, &mut mirror.last_synced);
        mirror.ref_status.insert(ref_name.to_string(), status);
        match collection
            .put(backend, cipher, mirror_id, &mirror, etag.as_deref())
            .await?
        {
            Cas::Ok => return Ok(()),
            Cas::Conflict if attempt < UPDATE_MAX_RETRIES => {
                gitquarry_core::storage::cas_retry_backoff(attempt).await;
                continue;
            }
            Cas::Conflict => break,
        }
    }
    bail!(
        "too many CAS conflicts updating mirror '{mirror_id}' ref '{ref_name}' (giving up after {UPDATE_MAX_RETRIES} attempts)"
    )
}
