use gitquarry_artifacts::domain::package::{self, NewPackageFile, NewPackageVersion};
use gitquarry_artifacts::storage::package as storage_package;
use s3db::{LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;

fn test_backend_and_cipher(root: &std::path::Path) -> (Arc<dyn s3db::Backend>, s3db_crypto::Cipher) {
    let backend: Arc<dyn s3db::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[97u8; s3db_crypto::SALT_LEN]).unwrap();
    (backend, cipher)
}

#[tokio::test]
async fn publish_version_stores_files_as_content_addressed_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let tarball = dir.path().join("left-pad-1.0.0.tgz");
    tokio::fs::write(&tarball, b"tarball bytes").await.unwrap();

    let published = package::publish_version(
        backend.as_ref(),
        &cipher,
        "npm",
        "left-pad",
        NewPackageVersion {
            version: "1.0.0",
            files: vec![NewPackageFile {
                filename: "left-pad-1.0.0.tgz",
                source: &tarball,
            }],
            format_metadata: serde_json::json!({"name": "left-pad"}),
            uploaded_by: "alice",
        },
    )
    .await
    .unwrap();

    assert_eq!(published.files.len(), 1);
    assert_eq!(published.files[0].size, 13);

    // The package itself gets created implicitly on first publish.
    let (meta, _etag) = storage_package::get_package(backend.as_ref(), &cipher, "npm", "left-pad")
        .await
        .unwrap();
    assert_eq!(meta.unwrap().name, "left-pad");
}

#[tokio::test]
async fn publish_version_rejects_republishing_the_same_version() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let tarball = dir.path().join("pkg.tgz");
    tokio::fs::write(&tarball, b"v1").await.unwrap();

    fn new(tarball: &std::path::Path) -> NewPackageVersion<'_> {
        NewPackageVersion {
            version: "1.0.0",
            files: vec![NewPackageFile {
                filename: "pkg.tgz",
                source: tarball,
            }],
            format_metadata: serde_json::json!({}),
            uploaded_by: "alice",
        }
    }
    package::publish_version(backend.as_ref(), &cipher, "npm", "pkg", new(&tarball))
        .await
        .unwrap();

    assert!(
        package::publish_version(backend.as_ref(), &cipher, "npm", "pkg", new(&tarball))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn fetch_version_returns_none_for_a_nonexistent_version() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    assert!(
        package::fetch_version(backend.as_ref(), &cipher, "npm", "pkg", "9.9.9")
            .await
            .unwrap()
            .is_none()
    );
}
