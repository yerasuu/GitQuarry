use gitquarry_artifacts::storage::blob;
use s3db::{LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;

fn test_backend_and_cipher(root: &std::path::Path) -> (Arc<dyn s3db::Backend>, s3db_crypto::Cipher) {
    let backend: Arc<dyn s3db::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[71u8; s3db_crypto::SALT_LEN]).unwrap();
    (backend, cipher)
}

#[tokio::test]
async fn upload_then_download_roundtrips_the_same_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());

    let source = dir.path().join("source.bin");
    tokio::fs::write(&source, b"hello world").await.unwrap();

    let (digest, len) = blob::upload_blob(backend.as_ref(), &cipher, &source).await.unwrap();
    assert_eq!(len, 11);
    assert!(blob::blob_exists(backend.as_ref(), &digest).await.unwrap());

    let dest = dir.path().join("dest.bin");
    blob::download_blob(backend.as_ref(), &cipher, &digest, &dest)
        .await
        .unwrap();
    assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"hello world");
}

#[tokio::test]
async fn uploading_identical_content_twice_is_a_dedup_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());

    let source = dir.path().join("source.bin");
    tokio::fs::write(&source, b"same content").await.unwrap();

    let (digest1, _) = blob::upload_blob(backend.as_ref(), &cipher, &source).await.unwrap();
    let (digest2, _) = blob::upload_blob(backend.as_ref(), &cipher, &source).await.unwrap();
    assert_eq!(digest1, digest2);
}

#[tokio::test]
async fn blob_exists_is_false_for_a_never_uploaded_digest() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, _cipher) = test_backend_and_cipher(dir.path());
    assert!(!blob::blob_exists(backend.as_ref(), &"a".repeat(64)).await.unwrap());
}
