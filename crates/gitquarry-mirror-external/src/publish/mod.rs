//! Publish mirrors: a private repo published, filtered, to a public remote —
//! `docs/publish-mirror-concept.md`. The entity, the filter, the gates, the
//! dry-run preview (phase 0), the snapshot publish run (phase 1) and the
//! history publish run (phase 2, [`history`]), both behind [`run`]. The
//! rewrite and the preview compute object ids in memory and write nothing;
//! only [`run`] writes objects, into a repository of its own, and pushes
//! from there.
//!
//! Hand-built collection rather than `#[derive(Entity)]`, for the same
//! reason as [`crate::access`]: the collection lives under the repo's own
//! `Uid`, known only at runtime.

pub mod audit;
pub mod epoch;
pub mod filter;
pub mod gc;
mod history;
pub mod inverse;
pub mod log;
mod mapping;
mod objects;
pub mod preview;
pub mod rewrite;
pub mod run;
mod scratch;
mod secrets;
mod snapshot;

use crate::validate::{
    validate_identity_email, validate_identity_name, validate_mirror_label, validate_mirror_token, validate_mirror_url,
    validate_publish_patterns, validate_publish_ref_pattern, validate_sha, validate_spdx_id, validate_tree_path,
};
use anyhow::{Result, bail};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{Cas, REPOS_PREFIX};
use s3db::Collection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub use filter::{FilterConfig, FilterMode};

/// Ref namespaces that are never published, whatever a mirror declares.
/// [`PublishMirror::publish_branches`]/[`PublishMirror::publish_tags`] only
/// ever match under `refs/heads/` and `refs/tags/`, so none of these is
/// reachable through them; the list is checked again on every selected ref
/// so that a future change to that matching can't open one by accident.
pub const NEVER_PUBLISHED_REF_PREFIXES: [&str; 3] = ["refs/merge-requests/", "refs/notes/", "refs/gitquarry/"];

/// What an author or committer whose email isn't in the identity map gets.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnmappedPolicy {
    Passthrough,
    /// The default: the module exists for "public licence, private company",
    /// where an internal address on a public commit is itself a leak.
    #[default]
    Block,
}

/// One private identity and what it becomes on the public side. Matched on
/// `private`, case-insensitively; `name`, when set, replaces the name too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityMapping {
    pub private: String,
    pub public: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// A secret-scan finding dismissed once as a false positive, keyed by the
/// exact content it was raised on: a later change to the same path is a new
/// blob and is scanned again. `path` is empty for a commit message, whose
/// `sha` is then the private commit's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretAllow {
    pub path: String,
    pub sha: String,
}

/// What a publish run puts on the public side.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublishMode {
    /// One commit per publish holding the filtered tree of each published
    /// branch's head, on top of the previous snapshot: no private history,
    /// no mapping. The right mode for a repo whose history is itself the
    /// sensitive part. Branches only: `publish_tags` is not read.
    #[default]
    Snapshot,
    /// The whole history reachable from the published refs, rewritten
    /// commit by commit, deterministically: the same private history under
    /// the same filter always gives the same public ids, so every publish
    /// within one [`PublishMirror::filter_epoch`] is a fast-forward of the
    /// last. Tags matching `publish_tags` are published too.
    History,
}

/// The forge credential of a publish mirror. Its own type so that `Debug`
/// (a test failure, a `tracing` field) can never print it; s3db seals it at
/// rest like every other field, as [`crate::access::ExternalMirror::token`].
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MirrorToken(String);

impl MirrorToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for MirrorToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() {
            "MirrorToken(none)"
        } else {
            "MirrorToken(<redacted>)"
        })
    }
}

/// Author and committer of every snapshot commit. A snapshot squashes many
/// private commits into one, so no private author is the right one to name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Publisher {
    pub name: String,
    pub email: String,
}

/// One publish mirror of a repo.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishMirror {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// The public remote, checked against the instance's host allowlist at
    /// save time and again at every preview and every publish run.
    #[serde(default)]
    pub remote_url: String,
    #[serde(default)]
    pub token: MirrorToken,
    /// Whether the periodic sweep publishes this mirror. "Publish now"
    /// works either way.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: PublishMode,
    /// Bumped, by one and only through a confirmed edit, whenever what a
    /// history publish outputs changes (see [`epoch`]). A new epoch
    /// rewrites the whole history and is the only way a publish forces.
    #[serde(default)]
    pub filter_epoch: u32,
    /// `None` blocks every snapshot run at the identity gate.
    #[serde(default)]
    pub publisher: Option<Publisher>,
    /// The authoritative half of the filter; `.gitquarry/publish.toml` in the
    /// source ref can only add to it (see [`filter::RepoFilterFile`]).
    #[serde(default)]
    pub filter: FilterConfig,
    /// Short branch names or `*`/`**` globs, matched under `refs/heads/`.
    #[serde(default)]
    pub publish_branches: BTreeSet<String>,
    /// Short tag names or globs, matched under `refs/tags/`.
    #[serde(default)]
    pub publish_tags: BTreeSet<String>,
    #[serde(default)]
    pub identities: Vec<IdentityMapping>,
    #[serde(default)]
    pub unmapped: UnmappedPolicy,
    /// The SPDX expression the published tree must declare, if any.
    #[serde(default)]
    pub spdx: Option<String>,
    #[serde(default)]
    pub secret_allowlist: Vec<SecretAllow>,
}

/// Every field of `mirror` that came from a form, checked before it is stored.
pub fn validate_publish_mirror(mirror: &PublishMirror) -> Result<()> {
    validate_mirror_url(&mirror.remote_url)?;
    validate_mirror_label(&mirror.label)?;
    if !mirror.token.is_empty() {
        validate_mirror_token(mirror.token.expose())?;
    } else if mirror.enabled {
        bail!("an enabled publish mirror needs a token for its public remote");
    }
    if let Some(publisher) = &mirror.publisher {
        validate_identity_name(&publisher.name)?;
        validate_identity_email(&publisher.email)?;
    }
    validate_publish_patterns(&mirror.filter.include)?;
    validate_publish_patterns(&mirror.filter.exclude)?;
    for pattern in mirror.publish_branches.iter().chain(&mirror.publish_tags) {
        validate_publish_ref_pattern(pattern)?;
    }
    let mut seen = BTreeSet::new();
    for identity in &mirror.identities {
        validate_identity_email(&identity.private)?;
        validate_identity_email(&identity.public)?;
        if let Some(name) = &identity.name {
            validate_identity_name(name)?;
        }
        if !seen.insert(identity.private.to_ascii_lowercase()) {
            bail!("identity {} is mapped twice", identity.private);
        }
    }
    if let Some(spdx) = &mirror.spdx {
        validate_spdx_id(spdx)?;
    }
    for allow in &mirror.secret_allowlist {
        validate_tree_path(&allow.path)?;
        validate_sha(&allow.sha)?;
    }
    Ok(())
}

fn publish_mirrors_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/publish-mirrors"))
}

/// Creates a new publish mirror, minting its id: whatever `mirror.id` held
/// is overwritten, so this can never overwrite an existing one.
pub async fn create(ctx: &AppContext, uid: &Uid, mut mirror: PublishMirror) -> Result<PublishMirror> {
    validate_publish_mirror(&mirror)?;
    check_host_allowed(ctx, &mirror.remote_url).await?;
    mirror.id = crate::access::generate_mirror_id();
    mirror.filter_epoch = 0;
    match publish_mirrors_collection(uid)
        .put(ctx.backend().as_ref(), &ctx.cipher, &mirror.id, &mirror, None)
        .await?
    {
        Cas::Ok => {
            crate::domain::refresh_mirrors_enabled_index(ctx, uid).await?;
            Ok(mirror)
        }
        Cas::Conflict => bail!("publish mirror id collision, try again"),
    }
}

/// `Ok(Cas::Conflict)` on a lost CAS race, like every other settings-tab
/// save, so the web layer can show its "edited concurrently" flash.
///
/// # Errors
///
/// [`epoch::EpochBumpRequired`] for an edit that changes what a history
/// publish outputs without bumping [`PublishMirror::filter_epoch`]; any
/// validation or host-gate refusal; a bucket error.
pub async fn update(ctx: &AppContext, uid: &Uid, mirror: &PublishMirror, expected_etag: &str) -> Result<Cas> {
    validate_publish_mirror(mirror)?;
    check_host_allowed(ctx, &mirror.remote_url).await?;
    let Some((stored, _etag)) = get(ctx, uid, &mirror.id).await? else {
        bail!("no such publish mirror");
    };
    epoch::check_update(&stored, mirror, &log::get(ctx, uid, &mirror.id).await?)?;
    let cas = publish_mirrors_collection(uid)
        .put(
            ctx.backend().as_ref(),
            &ctx.cipher,
            &mirror.id,
            mirror,
            Some(expected_etag),
        )
        .await?;
    if cas == Cas::Ok {
        if mirror.filter_epoch != stored.filter_epoch {
            epoch::record_bump(ctx, uid, &mirror.id, stored.filter_epoch).await;
        }
        crate::domain::refresh_mirrors_enabled_index(ctx, uid).await?;
    }
    Ok(cas)
}

pub async fn get(ctx: &AppContext, uid: &Uid, mirror_id: &str) -> Result<Option<(PublishMirror, String)>> {
    let (mirror, etag) = publish_mirrors_collection(uid)
        .get(ctx.backend().as_ref(), &ctx.cipher, mirror_id)
        .await?;
    Ok(mirror.zip(etag))
}

/// Every publish mirror of the repo with its etag, sorted by label. A repo
/// has a handful at most, so this is one small GET per mirror.
pub async fn list_with_etags(ctx: &AppContext, uid: &Uid) -> Result<Vec<(PublishMirror, String)>> {
    let collection = publish_mirrors_collection(uid);
    let backend = ctx.backend();
    let ids = collection.list_names(backend.as_ref()).await?;
    let mut mirrors = Vec::with_capacity(ids.len());
    for id in ids {
        if let (Some(mirror), Some(etag)) = collection
            .get::<PublishMirror>(backend.as_ref(), &ctx.cipher, &id)
            .await?
        {
            mirrors.push((mirror, etag));
        }
    }
    mirrors.sort_by(|(a, _), (b, _)| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
    Ok(mirrors)
}

/// Deletes the mirror and its run log; what it already published stays on
/// the public remote.
///
/// # Errors
///
/// Fails if the bucket refuses the delete; a mirror that is already gone is
/// not an error.
pub async fn delete(ctx: &AppContext, uid: &Uid, mirror_id: &str) -> Result<()> {
    publish_mirrors_collection(uid)
        .delete(ctx.backend().as_ref(), &ctx.cipher, mirror_id)
        .await?;
    log::delete(ctx, uid, mirror_id).await?;
    crate::domain::refresh_mirrors_enabled_index(ctx, uid).await
}

/// The host gate (`docs/publish-mirror-concept.md` §5): the same instance
/// allowlist and the same rule as an external mirror.
async fn check_host_allowed(ctx: &AppContext, remote_url: &str) -> Result<()> {
    crate::validate::check_host_allowed(remote_url, &allowed_hosts(ctx).await?)
}

async fn allowed_hosts(ctx: &AppContext) -> Result<Vec<String>> {
    let (settings, _etag) =
        gitquarry_core::storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    Ok(crate::validate::effective_allowed_hosts(&settings))
}
