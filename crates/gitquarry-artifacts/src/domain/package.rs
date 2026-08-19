//! Every mutation of a package/version goes through here — transport-
//! agnostic, `anyhow::Result`, no protocol-format (npm/raw) concerns; those
//! are later phases' adapters on top of this (`docs/artifacts-concept.md`
//! Phase 1).

use crate::storage::blob;
use crate::storage::package::{self, PackageFile, PackageMeta, PackageVersionMeta};
use anyhow::{Result, bail};
use s3db::{Backend, Cas, Cipher};
use std::path::Path;

pub struct NewPackageFile<'a> {
    pub filename: &'a str,
    pub source: &'a Path,
}

pub struct NewPackageVersion<'a> {
    pub version: &'a str,
    pub files: Vec<NewPackageFile<'a>>,
    pub format_metadata: serde_json::Value,
    pub uploaded_by: &'a str,
}

/// Unix seconds, hand-rolled rather than reusing `gitquarry-core`'s own
/// `storage::audit::format_unix_secs` — this crate never depends on that
/// crate. A real calendar-date format is later-phase polish, not a
/// foundation concern.
fn now_stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

/// Publishes a new version — `Err` if `version` already exists (versions
/// are immutable, mirroring the blob store's own "an existing key is
/// already correct content" discipline one layer down). Each file's blob is
/// uploaded content-addressed and deduplicated automatically
/// (`blob::upload_blob`'s own existence check) — republishing identical
/// bytes under a new version costs no extra storage.
pub async fn publish_version(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
    new: NewPackageVersion<'_>,
) -> Result<PackageVersionMeta> {
    if package::get_package_version(backend, cipher, format, name, new.version)
        .await?
        .0
        .is_some()
    {
        bail!(
            "version {} of {name} already exists — versions are immutable",
            new.version
        );
    }
    let mut files = Vec::with_capacity(new.files.len());
    for f in &new.files {
        let (sha256, size) = blob::upload_blob(backend, cipher, f.source).await?;
        files.push(PackageFile {
            filename: f.filename.to_string(),
            sha256,
            size,
        });
    }
    let stamp = now_stamp();
    let version_meta = PackageVersionMeta {
        version: new.version.to_string(),
        files,
        format_metadata: new.format_metadata,
        uploaded_by: new.uploaded_by.to_string(),
        uploaded_at: stamp.clone(),
    };
    if package::put_package_version(backend, cipher, format, name, &version_meta, None).await? != Cas::Ok {
        bail!(
            "concurrent publish of the same version {} of {name} — retry",
            new.version
        );
    }
    // Best-effort, tolerating a concurrent first-publish for this package
    // name as a harmless race — whichever write wins, the package now
    // exists either way. Same "tolerate conflict" shape
    // `docs/package-registry-concept.md`'s own `publish_version` design
    // decision describes.
    if package::get_package(backend, cipher, format, name).await?.0.is_none() {
        let meta = PackageMeta {
            name: name.to_string(),
            created_at: stamp,
        };
        let _ = package::put_package(backend, cipher, format, name, &meta, None).await;
    }
    Ok(version_meta)
}

pub async fn fetch_version(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
    version: &str,
) -> Result<Option<PackageVersionMeta>> {
    Ok(package::get_package_version(backend, cipher, format, name, version)
        .await?
        .0)
}

/// Namespace-`Owner`-only in a real deployment (the HTTP adapter checks
/// this via `core_client::CoreClient::namespace_permission` before calling
/// here — this domain layer itself has no permission concept, same
/// transport-agnostic split every other `domain::*` module in this
/// codebase already follows).
pub async fn delete_package(backend: &dyn Backend, cipher: &Cipher, format: &str, name: &str) -> Result<()> {
    package::delete_package(backend, cipher, format, name).await
}
