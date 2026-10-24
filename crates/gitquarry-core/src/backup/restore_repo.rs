//! Brings one repo back from a backup: the common case of a repo purged by
//! mistake, without writing anything of the rest of the instance.
//!
//! Each half of a backup supplies what only it holds:
//!
//! - The replica ([`super::replica`]) holds the repo's git data, LFS objects
//!   and release files at their own keys, sealed under this bucket's own data
//!   key, so they go back as raw ciphertext, with no decrypt.
//! - A snapshot ([`super::snapshot`]) holds the repo's entities
//!   (`.conf/config.json`, merge requests, LFS locks, mirror configs) as they
//!   were at that run, and is re-sealed back under each live key. The replica
//!   carries them too, but only as of the last run, so it is the fallback
//!   when no snapshot holds the repo at all (snapshots switched off).
//!
//! `refs.json` is read before anything is written and written last, and only
//! when every other object made it (kb note #30): refs pointing at a pack not
//! yet copied is a repo every clone fails on, while packs without refs are
//! just unreachable data the next run overwrites.
//!
//! What it leaves alone: permission grants live on the users and groups
//! (`User::repos`), which a purge never touches; the repo's name claim and
//! its owner/visible/permission indexes are derived, and rebuilding them is
//! `gitquarry backfill-repo-index`'s job (kb decision #7). Nothing is ever
//! deleted from the live bucket.

use crate::backup::inventory::{self, Class};
use crate::backup::pager::KeyPages;
use crate::backup::replica::DEFAULT_CONCURRENCY;
use crate::backup::snapshot;
use crate::crypto::Cipher;
use crate::storage::entities::RepoConfig;
use crate::storage::remote::MANIFEST_SUFFIX;
use crate::storage::uid::Uid;
use crate::storage::{Backend, Cas, INDEX_PREFIX, REPOS_PREFIX, get_json, settings};
use anyhow::{Context, Result, bail};
use futures::future;
use futures::stream::{self, Stream, StreamExt, TryStreamExt};
use std::sync::Arc;

/// Same cut-off as the replica's: above it an object streams through a
/// scratch file instead of memory.
const INLINE_MAX_BYTES: u64 = 1 << 20;

/// Where a restore takes the repo's entity keys from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitySource {
    Snapshot(String),
    /// No snapshot holds the repo's config: the replica's copy, i.e. the
    /// state at the last backup run.
    Replica,
}

/// What [`restore_repo`] wrote, or under `dry_run` would write.
#[derive(Debug)]
pub struct RepoRestoreReport {
    pub entities_from: EntitySource,
    /// Packs, LFS objects, release files and unclassified keys copied from
    /// the replica. `refs.json` is not counted here.
    pub replica_objects: u64,
    pub replica_bytes: u64,
    pub entity_objects: u64,
    /// `false` when the replica holds no `refs.json` for the repo (it was
    /// never pushed to), or when it was withheld because of `failures`.
    pub refs_restored: bool,
    /// The backed-up config carried `deleted_at`: the repo was soft-deleted
    /// before its purge, and the restore clears it so the repo serves again.
    pub undeleted: bool,
    /// `key: error` per object that couldn't be restored. Non-empty means
    /// `refs.json` was not written and the repo isn't usable yet; running
    /// the restore again retries everything.
    pub failures: Vec<String>,
}

/// The keys of one repo, built once from its uid.
struct RepoKeys {
    prefix: String,
    config: String,
    refs: String,
    /// `.conf/index/mirrors-enabled/<uid>.json` lives outside the repo
    /// prefix and has no rebuild path (`backup::inventory` classifies it as
    /// an entity); without it a restored repo's enabled mirrors never sync.
    mirrors_index: String,
}

impl RepoKeys {
    fn new(uid: &Uid) -> Result<Self> {
        let prefix = format!("{REPOS_PREFIX}/{uid}/");
        Ok(RepoKeys {
            config: RepoConfig::COLLECTION.key(uid.as_str())?,
            refs: format!("{prefix}{MANIFEST_SUFFIX}"),
            mirrors_index: format!("{INDEX_PREFIX}/mirrors-enabled/{uid}.json"),
            prefix,
        })
    }
}

/// Restores repo `uid` into `live` from the backup at `backup` (a replica
/// destination, which also holds the snapshots). `snapshot_id` picks the
/// snapshot the entities come from; `None` takes the newest one holding the
/// repo's config, else the replica's own copy.
///
/// The caller must have validated `uid` (`validate::validate_repo_uid`).
/// Overwrites the repo's live objects; with `dry_run` it only reads.
///
/// # Errors
///
/// Before anything is written: the backup and `live` don't share their
/// `.conf/encryption.json` (the replica's ciphertext wouldn't open here),
/// the backup holds no config for the repo, or its config or `refs.json`
/// doesn't decrypt under `cipher`. A decrypt failure is never worked around.
/// After writing started: a failed listing, or a failure clearing
/// `deleted_at`. A single object's failure is reported in
/// [`RepoRestoreReport::failures`] instead, and withholds `refs.json`.
pub async fn restore_repo(
    backup: &Arc<dyn Backend>,
    live: &Arc<dyn Backend>,
    cipher: &Cipher,
    uid: &Uid,
    snapshot_id: Option<&str>,
    dry_run: bool,
) -> Result<RepoRestoreReport> {
    ensure_same_data_key(backup.as_ref(), live.as_ref()).await?;
    let keys = RepoKeys::new(uid)?;
    let entities_from = entity_source(backup, cipher, &keys.config, snapshot_id).await?;
    let source_prefix = match &entities_from {
        EntitySource::Snapshot(id) => format!("{}/", snapshot::snapshot_prefix(id)?),
        EntitySource::Replica => String::new(),
    };

    let (config, config_etag) =
        get_json::<RepoConfig>(backup.as_ref(), &format!("{source_prefix}{}", keys.config), cipher)
            .await
            .context("decrypting the repo's backed-up config")?;
    if config_etag.is_none() {
        bail!("the backup holds no config for repo {uid}: nothing to restore");
    }
    let (refs_sealed, refs_etag) = backup
        .get(&keys.refs)
        .await
        .context("reading the replica's refs.json")?;
    let refs = match refs_etag {
        Some(_) => {
            cipher
                .open(&keys.refs, &refs_sealed)
                .context("decrypting the replica's refs.json")?;
            Some(refs_sealed)
        }
        None => None,
    };

    let mut report = RepoRestoreReport {
        entities_from,
        replica_objects: 0,
        replica_bytes: 0,
        entity_objects: 0,
        refs_restored: false,
        undeleted: config.deleted_at.is_some(),
        failures: Vec::new(),
    };

    let blobs = keys_under(backup, &keys.prefix).try_filter(|(key, _)| {
        future::ready(
            *key != keys.refs
                && matches!(
                    inventory::classify(key),
                    Class::Git | Class::Lfs | Class::Releases | Class::Unknown
                ),
        )
    });
    let mut copies = std::pin::pin!(
        blobs
            .map_ok(|(key, size)| async move {
                let outcome = if dry_run {
                    Ok(size)
                } else {
                    copy_raw(backup.as_ref(), live.as_ref(), &key, size).await
                };
                Ok::<_, anyhow::Error>((key, outcome))
            })
            .try_buffer_unordered(DEFAULT_CONCURRENCY)
    );
    while let Some((key, outcome)) = copies.try_next().await? {
        match outcome {
            Ok(bytes) => {
                report.replica_objects += 1;
                report.replica_bytes += bytes;
            }
            Err(e) => report.failures.push(format!("{key}: {e:#}")),
        }
    }

    let from_snapshot = !source_prefix.is_empty();
    let source_prefix = source_prefix.as_str();
    let mut entities = std::pin::pin!(
        keys_under(backup, &format!("{source_prefix}{}", keys.prefix))
            .chain(keys_under(backup, &format!("{source_prefix}{}", keys.mirrors_index)))
            .try_filter_map(|(key, size)| {
                let live_key = key
                    .strip_prefix(source_prefix)
                    .filter(|live_key| inventory::classify(live_key) == Class::Entities)
                    .map(str::to_string);
                future::ready(Ok(live_key.map(|live_key| (key, live_key, size))))
            })
            .map_ok(|(key, live_key, size)| async move {
                let outcome = if dry_run {
                    Ok(size)
                } else if from_snapshot {
                    snapshot::restore_one(backup.as_ref(), cipher, live.as_ref(), cipher, &key, &live_key).await
                } else {
                    copy_raw(backup.as_ref(), live.as_ref(), &live_key, size).await
                };
                Ok::<_, anyhow::Error>((live_key, outcome))
            })
            .try_buffer_unordered(DEFAULT_CONCURRENCY)
    );
    while let Some((live_key, outcome)) = entities.try_next().await? {
        match outcome {
            Ok(_) => report.entity_objects += 1,
            Err(e) => report.failures.push(format!("{live_key}: {e:#}")),
        }
    }

    if !report.failures.is_empty() {
        return Ok(report);
    }
    if dry_run {
        report.refs_restored = refs.is_some();
        return Ok(report);
    }
    if report.undeleted {
        undelete(live.as_ref(), cipher, uid).await?;
    }
    if let Some(bytes) = refs {
        overwrite(live.as_ref(), &keys.refs, bytes)
            .await
            .context("writing refs.json")?;
        report.refs_restored = true;
    }
    Ok(report)
}

/// Read-only twin of `replica::ensure_same_data_key`, which would install
/// the config into an empty bucket: a dry run must not write, and a restore
/// into a bucket with another data key must fail before writing anything.
async fn ensure_same_data_key(backup: &dyn Backend, live: &dyn Backend) -> Result<()> {
    let key = s3db::bootstrap::ENCRYPTION_META_KEY;
    let (backup_conf, backup_etag) = backup
        .get(key)
        .await
        .with_context(|| format!("reading {key} from the backup"))?;
    let (live_conf, live_etag) = live
        .get(key)
        .await
        .with_context(|| format!("reading {key} from the live bucket"))?;
    if backup_etag.is_none() || live_etag.is_none() || backup_conf != live_conf {
        bail!(
            "the backup and the live bucket don't carry the same {key}, so the replica's objects \
             can't be opened here: a repo can only be restored from a replica of this same bucket"
        );
    }
    Ok(())
}

async fn entity_source(
    backup: &Arc<dyn Backend>,
    cipher: &Cipher,
    config_key: &str,
    snapshot_id: Option<&str>,
) -> Result<EntitySource> {
    if let Some(id) = snapshot_id {
        let key = format!("{}/{config_key}", snapshot::snapshot_prefix(id)?);
        let (_, etag) = backup.get(&key).await.context("reading the snapshot")?;
        if etag.is_none() {
            bail!("snapshot '{id}' holds no config for this repo");
        }
        return Ok(EntitySource::Snapshot(id.to_string()));
    }
    for manifest in snapshot::list(backup, cipher).await? {
        let key = format!("{}/{config_key}", snapshot::snapshot_prefix(&manifest.id)?);
        let (_, etag) = backup.get(&key).await.context("reading the snapshot")?;
        if etag.is_some() {
            return Ok(EntitySource::Snapshot(manifest.id));
        }
    }
    Ok(EntitySource::Replica)
}

fn keys_under(backend: &Arc<dyn Backend>, prefix: &str) -> impl Stream<Item = Result<(String, u64)>> + use<> {
    stream::try_unfold(KeyPages::new(Arc::clone(backend), prefix), |mut pages| async move {
        Ok(pages.next().await?.map(|entry| (entry, pages)))
    })
}

/// A purged repo was soft-deleted first (`domain::repo::purge` refuses
/// otherwise), so any backup taken in its grace window carries `deleted_at`,
/// and git ssh/http refuse to serve a repo with it set.
async fn undelete(live: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<()> {
    let (current, etag) = settings::get_repo_config(live, uid.as_str(), cipher)
        .await
        .context("reading the restored repo config")?;
    let config = RepoConfig {
        deleted_at: None,
        ..current
    };
    match settings::put_repo_config(live, uid.as_str(), &config, etag.as_deref(), cipher)
        .await
        .context("clearing deleted_at on the restored repo config")?
    {
        Cas::Ok => {}
        Cas::Conflict => bail!("the repo config changed while it was being restored; run the restore again"),
    }
    settings::set_deleted_index(live, cipher, uid, false).await;
    Ok(())
}

/// Copies one object at its own key, ciphertext as-is. Returns the bytes
/// written.
async fn copy_raw(src: &dyn Backend, dest: &dyn Backend, key: &str, size: u64) -> Result<u64> {
    if size > INLINE_MAX_BYTES {
        let tmp = tempfile::Builder::new()
            .prefix(".gitquarry-restore-")
            .tempfile()
            .context("creating a scratch file for the copy")?
            .into_temp_path();
        src.get_stream(key, &tmp).await.context("reading the backup object")?;
        dest.put_stream(key, &tmp).await.context("writing the live object")?;
        return Ok(tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(size));
    }
    let (bytes, etag) = src.get(key).await.context("reading the backup object")?;
    if etag.is_none() {
        bail!("vanished from the backup mid-restore");
    }
    overwrite(dest, key, bytes).await
}

/// Unconditional overwrite through CAS, so losing a write race is retried
/// instead of silently leaving the object un-restored.
async fn overwrite(dest: &dyn Backend, key: &str, bytes: Vec<u8>) -> Result<u64> {
    let written = bytes.len() as u64;
    let (_, mut etag) = dest.get(key).await.context("reading the live object")?;
    for _ in 0..3 {
        match dest
            .put_cas(key, bytes.clone(), etag.as_deref())
            .await
            .context("writing the live object")?
        {
            Cas::Ok => return Ok(written),
            Cas::Conflict => etag = dest.get(key).await.context("reading the live object")?.1,
        }
    }
    bail!("too many write conflicts restoring it (gave up after 3 attempts)")
}
