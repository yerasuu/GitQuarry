//! Mode B: a versioned, re-sealed copy of selected data under
//! `.conf/backups/<id>/`, keyed by the object's original key.
//!
//! What the replica ([`super::replica`]) can't give: history. A replica
//! overwrites a mutable object in place, so yesterday's `global.json`,
//! yesterday's user list, yesterday's repo config are gone the moment the
//! next run lands. A snapshot keeps each run's copy under its own id, which
//! is what "restore the config to how it was on Tuesday" actually needs.
//! It's only worth doing for data that's both mutable and small — which is
//! why the default selection here is the entity/config classes, not packs.
//!
//! ## Why this doesn't use `s3db::backup`
//!
//! `s3db::backup_collection` works one `Collection` at a time, which can't
//! reach a `Singleton` at all (`.conf/global.json`, the ssh host key, the
//! signing key — none of them are collections), and it would need a
//! hand-maintained list of every collection in the workspace, exactly the
//! staleness [`super::inventory`] exists to avoid. It also writes each
//! entity to `{prefix}/{name}.json` while its own restore reads back through
//! a `Collection` carrying the *original* suffix, so for a collection whose
//! suffix isn't `.json` — `RepoConfig`'s is `/.conf/config.json` — the
//! restore side finds zero entities and silently restores nothing. Working
//! at the key level sidesteps all of it, and picks up each collection's
//! `.deleted/`/`.history/` bookkeeping for free, since those are just keys
//! under the same prefix.
//!
//! ## Re-sealing, not copying
//!
//! Unlike the replica, the destination key differs from the source key, and
//! every object is sealed with its own key as AAD — so each object must be
//! opened under its source key and re-sealed under its snapshot key. That's
//! the cost of history, and it's why this is pointed at the small mutable set
//! rather than at everything.

use crate::backup::inventory::{self, Class};
use crate::backup::pager::{KeyPages, PAGE_SIZE};
use crate::backup::report::BackupRun;
use crate::crypto::{Cipher, CompressionStrategy};
use crate::storage::{Backend, Cas, get_json, put_json};
use anyhow::{Context, Result, bail};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Root of every snapshot. Classified [`Class::Never`] by
/// [`inventory::classify`], so a snapshot never captures a previous one.
pub const SNAPSHOT_ROOT: &str = ".conf/backups";

/// Where every snapshot's manifest lives. Inside [`SNAPSHOT_ROOT`], so a
/// destination bucket carries its own index of what it holds and a restore
/// needs nothing but the bucket; dot-prefixed, so it can never collide with
/// a snapshot id (`validate::validate_backup_id` rejects a leading `.`).
pub const MANIFEST_PREFIX: &str = ".conf/backups/.manifests/";

/// Objects at or under this size are re-sealed in memory; bigger ones go
/// through the streaming envelope. Same threshold, and same reasoning, as
/// the replica's own inline cut-off.
const INLINE_MAX_BYTES: u64 = 1 << 20;

/// Which classes a snapshot captures unless a caller says otherwise: the
/// small, mutable, overwrite-in-place data whose previous value the replica
/// can't preserve. Packs, LFS objects and release assets are deliberately
/// absent — they're immutable, so the replica already keeps every version
/// there has ever been, and re-sealing them here would double the cost of a
/// backup for no added recoverability.
pub const DEFAULT_CLASSES: [Class; 1] = [Class::Entities];

/// What one snapshot captured. Stored under [`MANIFEST_PREFIX`], a sibling
/// of every snapshot's own directory rather than inside one: listing the
/// snapshots then reads one small directory instead of every object of every
/// snapshot, and a snapshot's own key listing can't mistake a manifest for
/// one of its objects.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub id: String,
    pub started_at: u64,
    pub finished_at: u64,
    /// [`Class::label`]s captured, so a restore can say what it holds
    /// without walking every key first.
    pub classes: Vec<String>,
    pub objects: u64,
    pub bytes: u64,
    /// `false` if any object failed to capture. A partial snapshot is still
    /// restorable — it just isn't everything, and a restore says so instead
    /// of implying completeness.
    pub complete: bool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// `.conf/backups/<id>` — the directory every captured key hangs under.
pub(crate) fn snapshot_prefix(id: &str) -> Result<String> {
    // `id` lands straight in an object key, so it goes through the same
    // check every caller-supplied name does: an id like `../repos` wouldn't
    // make an odd-looking snapshot, it would write into live data.
    crate::validate::validate_backup_id(id)?;
    Ok(format!("{SNAPSHOT_ROOT}/{id}"))
}

fn manifest_key(id: &str) -> Result<String> {
    crate::validate::validate_backup_id(id)?;
    Ok(format!("{MANIFEST_PREFIX}{id}.json"))
}

/// Captures every key of the selected `classes` into snapshot `id` at
/// `dest`, re-sealed under `dest_cipher`.
///
/// `src` and `dest` may be the same backend (a snapshot alongside the live
/// data) or different ones; likewise the ciphers. Per-object failures are
/// counted in `run` and don't stop the pass — the manifest just comes back
/// `complete: false`.
#[allow(clippy::too_many_arguments)] // Two (backend, cipher) pairs plus the id, classes and run: every one of them is a genuinely independent input, and bundling them into a struct would only move the same list somewhere less obvious.
pub async fn capture(
    src: &Arc<dyn Backend>,
    src_cipher: &Arc<Cipher>,
    dest: &Arc<dyn Backend>,
    dest_cipher: &Arc<Cipher>,
    id: &str,
    classes: &[Class],
    concurrency: usize,
    run: &mut BackupRun,
) -> Result<SnapshotManifest> {
    if classes.contains(&Class::Bootstrap) {
        // `.conf/encryption.json` is the one object in the bucket that isn't
        // sealed under the data key — it's what *holds* that key, wrapped
        // under the passphrase. There's nothing to open and re-seal, and a
        // snapshot doesn't need it either way: its objects are sealed under
        // the destination's own cipher, so restoring them needs the
        // destination's encryption config, which the destination already has.
        // Carrying it is the replica's job (see `replica::ensure_same_data_key`).
        bail!("the encryption config can't be snapshotted (it isn't sealed under the data key)");
    }
    let prefix = snapshot_prefix(id)?;
    let started_at = now_secs();
    let mut manifest = SnapshotManifest {
        id: id.to_string(),
        started_at,
        finished_at: started_at,
        classes: classes.iter().map(|c| c.label().to_string()).collect(),
        objects: 0,
        bytes: 0,
        complete: true,
    };

    // Paged, not one flat listing: the entity set is small on a small
    // instance and millions of objects on a large one, and this pass has to
    // keep working in the second case.
    let batch_size = concurrency.max(1);
    let mut keys = KeyPages::new(Arc::clone(src), "");
    let mut batch: Vec<(String, u64)> = Vec::with_capacity(batch_size);
    loop {
        match keys.next().await? {
            Some((key, size)) => {
                if !classes.contains(&inventory::classify(&key)) {
                    continue;
                }
                batch.push((key, size));
                if batch.len() < batch_size {
                    continue;
                }
            }
            // Listing exhausted: flush whatever the last partial batch holds
            // and stop.
            None if batch.is_empty() => break,
            None => {}
        }

        let mut results = stream::iter(std::mem::take(&mut batch).into_iter().map(|(key, size)| {
            let src = Arc::clone(src);
            let src_cipher = Arc::clone(src_cipher);
            let dest = Arc::clone(dest);
            let dest_cipher = Arc::clone(dest_cipher);
            let dest_key = format!("{prefix}/{key}");
            async move {
                let outcome = reseal(
                    src.as_ref(),
                    &src_cipher,
                    dest.as_ref(),
                    &dest_cipher,
                    &key,
                    &dest_key,
                    size,
                )
                .await;
                (key, outcome)
            }
        }))
        .buffer_unordered(batch_size)
        .boxed();

        while let Some((key, outcome)) = results.next().await {
            let class = inventory::classify(&key);
            match outcome {
                Ok(Some(bytes)) => {
                    manifest.objects += 1;
                    manifest.bytes += bytes;
                    run.count(class, Some(bytes));
                }
                Ok(None) => run.count(class, None),
                Err(e) => {
                    manifest.complete = false;
                    run.count_failure(class, &key, &format!("{e:#}"));
                }
            }
        }
    }

    manifest.finished_at = now_secs();
    put_json(dest.as_ref(), &manifest_key(id)?, &manifest, None, dest_cipher)
        .await
        .context("writing the snapshot manifest")?;
    Ok(manifest)
}

/// Opens one object under `src_key` and re-seals it under `dest_key`.
/// `Ok(None)` means there was nothing to do — the object vanished mid-run,
/// or this snapshot id already holds it (a re-run of the same id).
async fn reseal(
    src: &dyn Backend,
    src_cipher: &Cipher,
    dest: &dyn Backend,
    dest_cipher: &Cipher,
    src_key: &str,
    dest_key: &str,
    size: u64,
) -> Result<Option<u64>> {
    if size > INLINE_MAX_BYTES {
        return reseal_streaming(src, src_cipher, dest, dest_cipher, src_key, dest_key).await;
    }
    let (sealed, etag) = src.get(src_key).await.context("reading the source object")?;
    if etag.is_none() {
        // Deleted between the listing and now. The live bucket no longer has
        // it, so neither should the snapshot.
        return Ok(None);
    }
    let plain = src_cipher
        .open(src_key, &sealed)
        .context("decrypting the source object")?;
    let resealed = dest_cipher
        .seal(dest_key, &plain, CompressionStrategy::None)
        .context("re-encrypting for the snapshot")?;
    let written = resealed.len() as u64;
    // Create-only: a snapshot id is written once. A conflict means this id
    // already holds this key (a re-run after a partial failure), which is
    // not an error -- the object is already captured.
    match dest
        .put_cas(dest_key, resealed, None)
        .await
        .context("writing the snapshot object")?
    {
        Cas::Ok => Ok(Some(written)),
        Cas::Conflict => Ok(None),
    }
}

/// The same re-seal for an object too big to hold in memory. Which envelope
/// it's in isn't knowable from the key — small JSON uses `Cipher::seal`,
/// packs and LFS objects use `Cipher::seal_stream`, and opening one with the
/// other's routine fails outright rather than producing garbage — so it's
/// detected from the object's own first bytes, the same way
/// `storage::remote::Remote::reencrypt_object` does it.
async fn reseal_streaming(
    src: &dyn Backend,
    src_cipher: &Cipher,
    dest: &dyn Backend,
    dest_cipher: &Cipher,
    src_key: &str,
    dest_key: &str,
) -> Result<Option<u64>> {
    let sealed_tmp = tempfile::Builder::new()
        .prefix(".gitquarry-snapshot-")
        .tempfile()
        .context("creating a scratch file")?
        .into_temp_path();
    src.get_stream(src_key, &sealed_tmp)
        .await
        .context("reading the source object")?;

    let mut header = [0u8; 8];
    let header_len = {
        use tokio::io::AsyncReadExt;
        let mut f = tokio::fs::File::open(&sealed_tmp)
            .await
            .context("reopening the downloaded object")?;
        f.read(&mut header).await.context("peeking the downloaded object")?
    };

    let plain_tmp = tempfile::Builder::new()
        .prefix(".gitquarry-snapshot-")
        .tempfile()
        .context("creating a scratch file")?
        .into_temp_path();
    if Cipher::is_stream_sealed(&header[..header_len]) {
        src_cipher.open_stream(src_key, &sealed_tmp, &plain_tmp).await?;
    } else {
        let sealed = tokio::fs::read(&sealed_tmp)
            .await
            .context("reading the downloaded object")?;
        let plain = src_cipher.open(src_key, &sealed)?;
        tokio::fs::write(&plain_tmp, plain)
            .await
            .context("writing the decrypted object")?;
    }

    let resealed_tmp = tempfile::Builder::new()
        .prefix(".gitquarry-snapshot-")
        .tempfile()
        .context("creating a scratch file")?
        .into_temp_path();
    dest_cipher.seal_stream(dest_key, &plain_tmp, &resealed_tmp).await?;
    dest.put_stream(dest_key, &resealed_tmp)
        .await
        .context("writing the snapshot object")?;
    let written = tokio::fs::metadata(&resealed_tmp).await.map(|m| m.len()).unwrap_or(0);
    Ok(Some(written))
}

/// Every snapshot manifest at `backend`, newest first.
pub async fn list(backend: &Arc<dyn Backend>, cipher: &Cipher) -> Result<Vec<SnapshotManifest>> {
    let mut pages = KeyPages::new(Arc::clone(backend), MANIFEST_PREFIX);
    let mut keys: Vec<String> = Vec::new();
    while let Some((key, _size)) = pages.next().await? {
        keys.push(key);
    }
    keys.sort_unstable_by(|a, b| b.cmp(a));

    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        // An unreadable manifest is skipped rather than fatal: it must not
        // hide the snapshots that *are* readable, which is exactly when this
        // listing matters most.
        if let Ok((manifest, Some(_etag))) = get_json::<SnapshotManifest>(backend.as_ref(), &key, cipher).await {
            out.push(manifest);
        }
    }
    Ok(out)
}

/// What a [`restore`] did, or (with `dry_run`) would do.
#[derive(Debug, Default)]
pub struct RestoreReport {
    pub manifest: SnapshotManifest,
    /// Keys written back (or that would be, under `dry_run`).
    pub restored: u64,
    pub bytes: u64,
    /// `key: error` lines for objects that couldn't be restored.
    pub failures: Vec<String>,
}

/// Writes snapshot `id` back over the live data at `dest`, re-sealed under
/// `dest_cipher` at each object's original key.
///
/// Destructive by nature — it overwrites whatever is live now. `dry_run`
/// counts what it would touch and writes nothing; the caller
/// (`gitquarry restore`) is what makes the operator confirm.
#[allow(clippy::too_many_arguments)] // Same as `capture`: two (backend, cipher) pairs are the real shape of this operation.
pub async fn restore(
    snapshot: &Arc<dyn Backend>,
    snapshot_cipher: &Cipher,
    dest: &dyn Backend,
    dest_cipher: &Cipher,
    id: &str,
    dry_run: bool,
) -> Result<RestoreReport> {
    let (manifest, etag) = get_json::<SnapshotManifest>(snapshot.as_ref(), &manifest_key(id)?, snapshot_cipher)
        .await
        .context("reading the snapshot manifest")?;
    if etag.is_none() {
        bail!("no snapshot '{id}' found at {SNAPSHOT_ROOT}/");
    }

    let prefix = format!("{}/", snapshot_prefix(id)?);
    let mut report = RestoreReport {
        manifest,
        ..Default::default()
    };

    let mut keys = KeyPages::new(Arc::clone(snapshot), &prefix);
    while let Some((snapshot_key, size)) = keys.next().await? {
        let Some(live_key) = snapshot_key.strip_prefix(&prefix) else {
            continue;
        };
        if dry_run {
            report.restored += 1;
            report.bytes += size;
            continue;
        }
        match restore_one(
            snapshot.as_ref(),
            snapshot_cipher,
            dest,
            dest_cipher,
            &snapshot_key,
            live_key,
        )
        .await
        {
            Ok(bytes) => {
                report.restored += 1;
                report.bytes += bytes;
            }
            Err(e) => report.failures.push(format!("{live_key}: {e:#}")),
        }
    }
    Ok(report)
}

pub(crate) async fn restore_one(
    snapshot: &dyn Backend,
    snapshot_cipher: &Cipher,
    dest: &dyn Backend,
    dest_cipher: &Cipher,
    snapshot_key: &str,
    live_key: &str,
) -> Result<u64> {
    let (sealed, etag) = snapshot
        .get(snapshot_key)
        .await
        .context("reading the snapshot object")?;
    if etag.is_none() {
        bail!("vanished from the snapshot mid-restore");
    }
    let plain = snapshot_cipher
        .open(snapshot_key, &sealed)
        .context("decrypting the snapshot object")?;
    let resealed = dest_cipher
        .seal(live_key, &plain, CompressionStrategy::None)
        .context("re-encrypting for the live key")?;
    let written = resealed.len() as u64;

    // Read the live etag immediately before writing, and re-read on
    // conflict: a restore is a deliberate overwrite, but it still must not
    // lose the write race silently and leave this object un-restored.
    let (_, mut live_etag) = dest.get(live_key).await.context("reading the live object")?;
    for _ in 0..3 {
        match dest
            .put_cas(live_key, resealed.clone(), live_etag.as_deref())
            .await
            .context("writing the live object")?
        {
            Cas::Ok => return Ok(written),
            Cas::Conflict => live_etag = dest.get(live_key).await?.1,
        }
    }
    bail!("too many write conflicts restoring it (gave up after 3 attempts)")
}

/// Deletes snapshot `id` outright — its manifest and every key under it.
/// Irreversible: only the retention sweep and an explicit operator command
/// call this.
pub async fn delete(backend: &Arc<dyn Backend>, id: &str) -> Result<usize> {
    let prefix = format!("{}/", snapshot_prefix(id)?);
    // Deleted a page at a time rather than by collecting every key first:
    // the same bounded-memory reason the capture pass pages, and
    // `delete_many` already chunks to each driver's own batch limit.
    let mut keys = KeyPages::new(Arc::clone(backend), &prefix);
    let mut batch: Vec<String> = Vec::with_capacity(PAGE_SIZE);
    let mut deleted = 0usize;
    loop {
        match keys.next().await? {
            Some((key, _size)) => {
                batch.push(key);
                if batch.len() < PAGE_SIZE {
                    continue;
                }
            }
            None if batch.is_empty() => break,
            None => {}
        }
        deleted += batch.len();
        backend
            .delete_many(&std::mem::take(&mut batch))
            .await
            .context("deleting the snapshot")?;
    }
    backend
        .delete_many(&[manifest_key(id)?])
        .await
        .context("deleting the snapshot manifest")?;
    Ok(deleted + 1)
}
