//! Mode A: a byte-for-byte replica of the bucket in another bucket (or
//! another directory), at the *same* keys.
//!
//! Same keys is the whole trick. Every object is sealed with its own key as
//! AAD, so copying it to a different key would mean decrypt-and-re-seal —
//! a full read, a full write, and CPU on every byte, every run. Copied to
//! the same key in a destination that carries the source's own
//! `.conf/encryption.json` (hence the same wrapped data key), the ciphertext
//! is valid exactly as it stands: no decrypt, no plaintext on disk, and the
//! destination is a bucket `gitquarry --bucket <replica>` can serve straight
//! out of.
//!
//! ## How a run stays bounded
//!
//! Source and destination are walked as two *paged* listings merged against
//! each other (they're both in ascending key order, and mode A keeps keys
//! identical, so this is an ordinary merge join). Memory is one page per
//! side regardless of how many objects the bucket holds — the alternative,
//! listing either side in full first, is what stops scaling at a few million
//! objects.
//!
//! ## What gets skipped
//!
//! Only keys that are present at the destination, the same size, **and**
//! immutable ([`inventory::is_immutable`]). A mutable object is re-copied
//! every run even when its size matches, because a same-size rewrite
//! (a counter, a status flag, a rename to a name of equal length) is
//! completely ordinary and size alone can't see it.
//!
//! ## What is never deleted
//!
//! Keys present at the destination and absent at the source are left alone.
//! That's not an oversight: it's what makes an accidental `repack`, an
//! erroneous repo purge, or a malicious deletion recoverable from the
//! replica instead of instantly mirrored into it. Removing them is a
//! separate, opt-in, grace-guarded prune.

use crate::backup::inventory::{self, Class, Selection};
use crate::backup::report::BackupRun;
use crate::storage::Backend;
use anyhow::{Context, Result, bail};
use futures::stream::{self, StreamExt};
use std::collections::VecDeque;
use std::sync::Arc;

/// Keys per listing page. Big enough that a large bucket doesn't spend its
/// life in LIST round trips, small enough that two pages in memory is
/// nothing. Matches S3's own natural page size.
const PAGE_SIZE: usize = 1000;

/// Objects at or under this size are copied through memory and compared
/// byte-for-byte; bigger ones stream through a scratch file and are compared
/// only by size. The line is drawn where reading both sides to compare stops
/// being obviously cheaper than just rewriting.
const INLINE_MAX_BYTES: u64 = 1 << 20;

/// Concurrent object copies. Same value (and same reasoning) as
/// `storage::remote::COPY_CONCURRENCY`: enough to keep the pipe busy,
/// not enough to open a connection per object in a bucket with thousands
/// of small ones.
pub const DEFAULT_CONCURRENCY: usize = 8;

/// Refuses to replicate into a destination whose encryption config isn't the
/// source's, and installs it when the destination is empty.
///
/// This has to run before any data is copied, and not only because a raw
/// ciphertext copy is undecryptable under a different data key:
/// `s3db::encryption_conf::ensure_safe_to_initialize` refuses to bootstrap a
/// fresh encryption config into a bucket that already holds objects, so a
/// destination filled with data *first* could never be given a config
/// afterwards — it would be permanently unreadable.
pub async fn ensure_same_data_key(src: &dyn Backend, dest: &dyn Backend) -> Result<()> {
    let key = s3db::bootstrap::ENCRYPTION_META_KEY;
    let (src_bytes, src_etag) = src
        .get(key)
        .await
        .with_context(|| format!("reading {key} from the source"))?;
    if src_etag.is_none() {
        bail!("the source bucket has no {key}: it was never initialized, so there's nothing to replicate");
    }

    let (dest_bytes, dest_etag) = dest
        .get(key)
        .await
        .with_context(|| format!("reading {key} from the destination"))?;
    if dest_etag.is_some() {
        // Byte equality is exactly the right test: this object holds the
        // salt and the wrapped data key, so identical bytes mean the same
        // data key, and anything else means every object copied at its own
        // key would fail to open at the destination.
        if dest_bytes != src_bytes {
            bail!(
                "the destination was initialized with a different encryption config ({key} \
                 differs), so objects copied at their own keys could never be decrypted there. \
                 A replica destination must either be empty or already carry the source's own \
                 {key}."
            );
        }
        return Ok(());
    }

    // No config at the destination. Empty destination: install it. Anything
    // else is some other bucket's data, and writing our config over it would
    // claim it as ours while leaving all of it unreadable.
    let probe = dest.list_page("", None, 1).await.context("listing the destination")?;
    if !probe.entries.is_empty() {
        bail!(
            "the destination already holds objects but has no {key}, so it isn't a gitquarry \
             bucket this instance can replicate into. Point the backup at an empty destination, \
             or at one already replicated from this same source."
        );
    }
    dest.put_cas(key, src_bytes, None)
        .await
        .with_context(|| format!("writing {key} to the destination"))?;
    Ok(())
}

/// Copies every selected object from `src` to `dest` at its own key,
/// recording what happened in `run`. See the module doc for the skip rule
/// and for why nothing is ever deleted.
///
/// `Err` only for a failure that makes the whole run meaningless (a listing
/// that fails); a single object's failure is counted in `run` and the pass
/// continues — one unreadable pack must not cost the other ten thousand.
pub async fn sync(
    src: &Arc<dyn Backend>,
    dest: &Arc<dyn Backend>,
    selection: Selection,
    concurrency: usize,
    run: &mut BackupRun,
) -> Result<()> {
    ensure_same_data_key(src.as_ref(), dest.as_ref()).await?;

    // The whole bucket, not a list of known prefixes: a prefix nobody
    // taught this module about is exactly the data `Class::Unknown` exists
    // to carry, and it can only be seen by looking everywhere.
    let mut src_keys = KeyPages::new(Arc::clone(src), "");
    let mut dest_keys = KeyPages::new(Arc::clone(dest), "");

    let batch = concurrency.max(1) * 4;
    let mut pending: Vec<Work> = Vec::with_capacity(batch);
    let mut src_next = src_keys.next().await?;
    let mut dest_next = dest_keys.next().await?;

    while let Some((src_key, src_size)) = src_next.clone() {
        let dest_size = match &dest_next {
            // Both listings are ascending, so a destination key that sorts
            // before this source key can only be one the source no longer
            // has: skip past it without touching it (see the module doc on
            // deletions) and re-compare.
            Some((dest_key, _)) if dest_key < &src_key => {
                dest_next = dest_keys.next().await?;
                continue;
            }
            Some((dest_key, size)) if dest_key == &src_key => Some(*size),
            _ => None,
        };

        if let Some(work) = plan(&src_key, src_size, dest_size, selection, run) {
            pending.push(work);
            if pending.len() >= batch {
                run_batch(src, dest, std::mem::take(&mut pending), run).await;
            }
        }

        if dest_size.is_some() {
            dest_next = dest_keys.next().await?;
        }
        src_next = src_keys.next().await?;
    }

    if !pending.is_empty() {
        run_batch(src, dest, pending, run).await;
    }
    Ok(())
}

/// One object still to copy.
struct Work {
    key: String,
    size: u64,
    class: Class,
    /// Whether the destination already has this key, which decides whether
    /// the small-object path can create it outright or has to overwrite.
    at_dest: bool,
}

/// Decides what to do with one source key, counting the "nothing to do"
/// cases straight into `run`. `None` means no copy is needed.
fn plan(key: &str, size: u64, dest_size: Option<u64>, selection: Selection, run: &mut BackupRun) -> Option<Work> {
    let class = inventory::classify(key);
    if !selection.includes(class) {
        return None;
    }
    if class == Class::Unknown {
        run.note_unknown(key);
    }
    // Present, same size, and never rewritten in place -> genuinely already
    // backed up. Every other combination gets re-copied; see the module doc.
    if dest_size == Some(size) && inventory::is_immutable(key) {
        run.count(class, None);
        return None;
    }
    Some(Work {
        key: key.to_string(),
        size,
        class,
        at_dest: dest_size.is_some(),
    })
}

async fn run_batch(src: &Arc<dyn Backend>, dest: &Arc<dyn Backend>, batch: Vec<Work>, run: &mut BackupRun) {
    let concurrent = batch.len();
    let mut results = stream::iter(batch.into_iter().map(|work| {
        let src = Arc::clone(src);
        let dest = Arc::clone(dest);
        async move {
            let outcome = copy_object(src.as_ref(), dest.as_ref(), &work).await;
            (work, outcome)
        }
    }))
    .buffer_unordered(concurrent)
    .boxed();

    while let Some((work, outcome)) = results.next().await {
        match outcome {
            Ok(Some(bytes)) => run.count(work.class, Some(bytes)),
            // The destination already had these exact bytes -- reached only
            // on the small-object path, which compares content rather than
            // trusting size.
            Ok(None) => run.count(work.class, None),
            Err(e) => run.count_failure(work.class, &work.key, &format!("{e:#}")),
        }
    }
}

/// Copies one object. `Ok(None)` means the destination already held
/// identical bytes and nothing was written; `Ok(Some(n))` means `n` bytes
/// were written.
async fn copy_object(src: &dyn Backend, dest: &dyn Backend, work: &Work) -> Result<Option<u64>> {
    if work.size <= INLINE_MAX_BYTES {
        return copy_small(src, dest, work).await;
    }
    copy_streaming(src, dest, work).await
}

/// Small objects go through memory, which buys an exact byte comparison: a
/// mutable object rewritten to the same value (very common — most config
/// saves change one field, many change none) costs a read on each side and
/// no write at all.
async fn copy_small(src: &dyn Backend, dest: &dyn Backend, work: &Work) -> Result<Option<u64>> {
    let (bytes, etag) = src.get(&work.key).await.context("reading the source object")?;
    if etag.is_none() {
        // Vanished between the listing and now (a repack, a purge). Not an
        // error: the source no longer has it, so neither does the backup.
        return Ok(None);
    }
    let dest_etag = if work.at_dest {
        let (dest_bytes, dest_etag) = dest.get(&work.key).await.context("reading the destination object")?;
        if dest_bytes == bytes {
            return Ok(None);
        }
        dest_etag
    } else {
        None
    };
    let written = bytes.len() as u64;
    match dest
        .put_cas(&work.key, bytes, dest_etag.as_deref())
        .await
        .context("writing the destination object")?
    {
        crate::storage::Cas::Ok => Ok(Some(written)),
        // Something else wrote this key at the destination between the read
        // above and this write. For a replica that's only possible if two
        // backups are running at once, and the other one wrote the same
        // source object -- so the bytes are already right and there's
        // nothing to retry.
        crate::storage::Cas::Conflict => Ok(None),
    }
}

/// Big objects (packs, LFS objects, release assets) stream through a scratch
/// file, so a multi-gigabyte pack never lands in memory. No CAS: `put_stream`
/// doesn't offer one, and doesn't need to — reaching here means the size
/// already differs or the object is mutable, both of which want an
/// unconditional overwrite.
async fn copy_streaming(src: &dyn Backend, dest: &dyn Backend, work: &Work) -> Result<Option<u64>> {
    let tmp = tempfile::Builder::new()
        .prefix(".gitquarry-backup-")
        .tempfile()
        .context("creating a scratch file for the copy")?
        .into_temp_path();
    src.get_stream(&work.key, &tmp)
        .await
        .context("reading the source object")?;
    dest.put_stream(&work.key, &tmp)
        .await
        .context("writing the destination object")?;
    let written = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(work.size);
    Ok(Some(written))
}

/// A paged key listing, handed out one `(key, size)` at a time.
///
/// Holds one page. Every driver that matters overrides `Backend::list_page`
/// natively (S3, local files, redb, plus the resilience and cache wrappers);
/// the ones that don't fall back to `s3db`'s default, which re-lists and
/// slices per page — correct, just not cheap, which is worth knowing before
/// pointing a very large bucket's backup at one of those drivers.
struct KeyPages {
    backend: Arc<dyn Backend>,
    prefix: String,
    buf: VecDeque<(String, u64)>,
    after: Option<String>,
    exhausted: bool,
}

impl KeyPages {
    fn new(backend: Arc<dyn Backend>, prefix: &str) -> Self {
        KeyPages {
            backend,
            prefix: prefix.to_string(),
            buf: VecDeque::new(),
            after: None,
            exhausted: false,
        }
    }

    async fn next(&mut self) -> Result<Option<(String, u64)>> {
        while self.buf.is_empty() && !self.exhausted {
            let page = self
                .backend
                .list_page(&self.prefix, self.after.as_deref(), PAGE_SIZE)
                .await
                .with_context(|| format!("listing '{}'", self.prefix))?;
            self.after = page.next_after.clone();
            self.exhausted = page.next_after.is_none();
            self.buf.extend(page.entries);
        }
        Ok(self.buf.pop_front())
    }
}
