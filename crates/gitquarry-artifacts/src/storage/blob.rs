//! Content-addressed blob storage shared across every package/version —
//! `blobs/<sha256>`. Built directly on `Backend::put_stream`/`get_stream` +
//! `Cipher::seal_stream`/`open_stream` (this crate never depends on
//! `gitquarry-core`, so there's no `storage::remote::Remote` to reuse — same
//! "no dependency on core" posture `gitquarry-federation`/`gitquarry-ci`
//! already establish), mirroring the exact "tempfile → hash → seal → commit"
//! discipline `gitquarry-core`'s own LFS object storage already proves
//! (`git::lfs::upload_object_impl`/`storage::remote::seal_and_upload`).

use anyhow::{Context, Result};
use s3db::{Backend, Cipher};
use sha2::{Digest, Sha256};
use std::path::Path;

const BLOBS_PREFIX: &str = "artifacts/blobs";

fn blob_key(sha256_hex: &str) -> String {
    format!("{BLOBS_PREFIX}/{sha256_hex}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `true` if a blob with this hash is already stored.
pub async fn blob_exists(backend: &dyn Backend, sha256_hex: &str) -> Result<bool> {
    Ok(!backend.list_prefix(&blob_key(sha256_hex)).await?.is_empty())
}

/// Streams `source`'s bytes into the content-addressed store, hashing while
/// streaming rather than requiring the caller to have hashed it already —
/// same discipline `git::lfs::upload_object_impl` uses. Returns the hex
/// sha256 (the blob's own key) and its byte length. Content-addressed, so
/// an existing blob under the computed key is already correct content —
/// dedup-skips the actual `put_stream` call, same as `git::lfs`'s own
/// existence check before uploading.
pub async fn upload_blob(backend: &dyn Backend, cipher: &Cipher, source: &Path) -> Result<(String, u64)> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(source).await.context("opening upload source")?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut len: u64 = 0;
    loop {
        let n = file.read(&mut buf).await.context("reading upload source")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    let digest = hex(&hasher.finalize());
    let key = blob_key(&digest);
    if !backend.list_prefix(&key).await?.is_empty() {
        return Ok((digest, len));
    }
    let sealed = tempfile::NamedTempFile::new().context("creating scratch tempfile")?;
    cipher
        .seal_stream(&key, source, sealed.path())
        .await
        .context("sealing blob for upload")?;
    backend
        .put_stream(&key, sealed.path())
        .await
        .context("uploading blob")?;
    Ok((digest, len))
}

/// Downloads and decrypts the blob keyed by `sha256_hex` to `dest`.
pub async fn download_blob(backend: &dyn Backend, cipher: &Cipher, sha256_hex: &str, dest: &Path) -> Result<()> {
    let key = blob_key(sha256_hex);
    let sealed = tempfile::NamedTempFile::new().context("creating scratch tempfile")?;
    backend
        .get_stream(&key, sealed.path())
        .await
        .context("downloading blob")?;
    cipher
        .open_stream(&key, sealed.path(), dest)
        .await
        .context("opening downloaded blob")?;
    Ok(())
}
