use gitquarry_artifacts::storage::package::{self, PackageMeta, PackageVersionMeta};
use s3db::{Cas, LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;

fn test_backend_and_cipher(root: &std::path::Path) -> (Arc<dyn s3db::Backend>, Arc<s3db_crypto::Cipher>) {
    let backend: Arc<dyn s3db::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(s3db_crypto::Cipher::derive("test passphrase", &[83u8; s3db_crypto::SALT_LEN]).unwrap());
    (backend, cipher)
}

#[tokio::test]
async fn put_then_get_package_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let meta = PackageMeta {
        name: "left-pad".to_string(),
        created_at: "123".to_string(),
    };
    assert_eq!(
        package::put_package(backend.as_ref(), &cipher, "npm", "left-pad", &meta, None)
            .await
            .unwrap(),
        Cas::Ok
    );
    let (fetched, etag) = package::get_package(backend.as_ref(), &cipher, "npm", "left-pad")
        .await
        .unwrap();
    assert_eq!(fetched.unwrap().name, "left-pad");
    assert!(etag.is_some());
}

#[tokio::test]
async fn get_package_is_none_for_a_never_published_name() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    assert!(
        package::get_package(backend.as_ref(), &cipher, "npm", "nope")
            .await
            .unwrap()
            .0
            .is_none()
    );
}

#[tokio::test]
async fn list_packages_only_lists_the_given_format() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let meta = |name: &str| PackageMeta {
        name: name.to_string(),
        created_at: "123".to_string(),
    };
    package::put_package(backend.as_ref(), &cipher, "npm", "a", &meta("a"), None)
        .await
        .unwrap();
    package::put_package(backend.as_ref(), &cipher, "npm", "b", &meta("b"), None)
        .await
        .unwrap();
    package::put_package(backend.as_ref(), &cipher, "raw", "c", &meta("c"), None)
        .await
        .unwrap();

    let npm_packages = package::list_packages(&backend, &cipher, "npm").await.unwrap();
    let mut names: Vec<_> = npm_packages.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["a", "b"]);
}

#[tokio::test]
async fn put_then_get_package_version_roundtrips_and_lists_only_that_package() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let version = |v: &str| PackageVersionMeta {
        version: v.to_string(),
        files: Vec::new(),
        format_metadata: serde_json::json!({}),
        uploaded_by: "alice".to_string(),
        uploaded_at: "123".to_string(),
    };
    package::put_package_version(backend.as_ref(), &cipher, "npm", "left-pad", &version("1.0.0"), None)
        .await
        .unwrap();
    package::put_package_version(backend.as_ref(), &cipher, "npm", "left-pad", &version("1.0.1"), None)
        .await
        .unwrap();
    package::put_package_version(backend.as_ref(), &cipher, "npm", "other-pkg", &version("1.0.0"), None)
        .await
        .unwrap();

    let (fetched, etag) = package::get_package_version(backend.as_ref(), &cipher, "npm", "left-pad", "1.0.0")
        .await
        .unwrap();
    assert_eq!(fetched.unwrap().uploaded_by, "alice");
    assert!(etag.is_some());

    let versions = package::list_package_versions(&backend, &cipher, "npm", "left-pad")
        .await
        .unwrap();
    let mut vs: Vec<_> = versions.iter().map(|v| v.version.as_str()).collect();
    vs.sort_unstable();
    assert_eq!(vs, vec!["1.0.0", "1.0.1"]);
}

#[tokio::test]
async fn tags_default_to_empty_and_roundtrip_once_set() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let (tags, etag) = package::get_tags(backend.as_ref(), &cipher, "npm", "left-pad")
        .await
        .unwrap();
    assert!(tags.0.is_empty());
    assert!(etag.is_none());

    let mut updated = package::PackageTags::default();
    updated.0.insert("latest".to_string(), "1.0.1".to_string());
    package::put_tags(backend.as_ref(), &cipher, "npm", "left-pad", &updated, etag.as_deref())
        .await
        .unwrap();

    let (fetched, _etag) = package::get_tags(backend.as_ref(), &cipher, "npm", "left-pad")
        .await
        .unwrap();
    assert_eq!(fetched.0.get("latest"), Some(&"1.0.1".to_string()));
}
