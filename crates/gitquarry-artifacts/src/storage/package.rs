//! Package/version metadata — content-addressed blob *references* live
//! here (see [`PackageFile`]); the blobs themselves are
//! [`super::blob`]'s own concern. Two `Collection` granularities, on
//! purpose: [`package_meta_collection`]/[`package_tags_collection`] scoped
//! per-*format* (so `list_names` naturally lists every package name in that
//! format, for [`list_packages`]), [`package_versions_collection`] scoped
//! per-*package* (so `list_names` naturally lists just that package's
//! versions, for [`list_package_versions`]) — a deliberate reordering of
//! `docs/artifacts-concept.md`'s original `pkg/<format>/<name>/meta.json`
//! sketch (this crate uses `pkg/<format>/meta/<name>` instead) so
//! `Collection::list_names` (which only ever lists one flat level) lands on
//! exactly the right scope for each list operation without a second filter
//! pass.

use anyhow::Result;
use s3db::{Backend, Cas, Cipher, Collection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PackageMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub created_at: String,
}

/// One content-addressed file belonging to a [`PackageVersionMeta`] — a
/// version can have more than one (npm's tarball, Maven's jar+pom+sources,
/// ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageFile {
    pub filename: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PackageVersionMeta {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub files: Vec<PackageFile>,
    /// Round-tripped verbatim — format-specific manifest fields (npm's
    /// `package.json`, ...) never leak into this generic layer. `Phase 2/3`
    /// adapters are the only readers/writers of this field's shape.
    #[serde(default)]
    pub format_metadata: serde_json::Value,
    #[serde(default)]
    pub uploaded_by: String,
    #[serde(default)]
    pub uploaded_at: String,
}

/// npm-style `dist-tags` (`"latest"` -> version) — generic enough for any
/// format that wants a mutable pointer to "the current version", not
/// npm-specific despite the name origin.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PackageTags(pub BTreeMap<String, String>);

fn package_meta_collection(format: &str) -> Collection {
    Collection::new_owned(format!("artifacts/pkg/{format}/meta"))
}

fn package_tags_collection(format: &str) -> Collection {
    Collection::new_owned(format!("artifacts/pkg/{format}/tags"))
}

fn package_versions_collection(format: &str, name: &str) -> Collection {
    Collection::new_owned(format!("artifacts/pkg/{format}/{name}/versions"))
}

pub async fn get_package(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
) -> Result<(Option<PackageMeta>, Option<String>)> {
    package_meta_collection(format).get(backend, cipher, name).await
}

pub async fn put_package(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
    meta: &PackageMeta,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    package_meta_collection(format)
        .put(backend, cipher, name, meta, expected_etag)
        .await
}

/// Every package currently published under `format`, unordered.
pub async fn list_packages(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>, format: &str) -> Result<Vec<PackageMeta>> {
    let collection = package_meta_collection(format);
    let names = collection.list_names(backend.as_ref()).await?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        if let (Some(meta), _etag) = collection
            .get::<PackageMeta>(backend.as_ref(), cipher.as_ref(), &name)
            .await?
        {
            out.push(meta);
        }
    }
    Ok(out)
}

pub async fn get_package_version(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
    version: &str,
) -> Result<(Option<PackageVersionMeta>, Option<String>)> {
    package_versions_collection(format, name)
        .get(backend, cipher, version)
        .await
}

pub async fn put_package_version(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
    version_meta: &PackageVersionMeta,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    package_versions_collection(format, name)
        .put(backend, cipher, &version_meta.version, version_meta, expected_etag)
        .await
}

/// Every version currently published for `name` under `format`, unordered —
/// callers sort by whatever ordering their format cares about (semver for
/// npm, lexical for raw, ...), not this generic layer's job.
pub async fn list_package_versions(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    format: &str,
    name: &str,
) -> Result<Vec<PackageVersionMeta>> {
    let collection = package_versions_collection(format, name);
    let versions = collection.list_names(backend.as_ref()).await?;
    let mut out = Vec::with_capacity(versions.len());
    for version in versions {
        if let (Some(meta), _etag) = collection
            .get::<PackageVersionMeta>(backend.as_ref(), cipher.as_ref(), &version)
            .await?
        {
            out.push(meta);
        }
    }
    Ok(out)
}

pub async fn get_tags(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
) -> Result<(PackageTags, Option<String>)> {
    package_tags_collection(format)
        .get_or_default(backend, cipher, name)
        .await
}

pub async fn put_tags(
    backend: &dyn Backend,
    cipher: &Cipher,
    format: &str,
    name: &str,
    tags: &PackageTags,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    package_tags_collection(format)
        .put(backend, cipher, name, tags, expected_etag)
        .await
}

/// Deletes only this package's own metadata subtree (its `meta`/`tags`
/// entries and every version under it) — blobs are namespace-shared and
/// never garbage-collected here, same tradeoff LFS already has (see
/// `docs/artifacts-concept.md`'s own "explicitly still out of scope" list).
pub async fn delete_package(backend: &dyn Backend, cipher: &Cipher, format: &str, name: &str) -> Result<()> {
    package_meta_collection(format).delete(backend, cipher, name).await;
    package_tags_collection(format).delete(backend, cipher, name).await;
    let versions_collection = package_versions_collection(format, name);
    for version in versions_collection.list_names(backend).await? {
        versions_collection.delete(backend, cipher, &version).await;
    }
    Ok(())
}
