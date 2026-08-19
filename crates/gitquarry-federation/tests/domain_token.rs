use gitquarry_federation::domain::token::*;
use s3db::Cipher;
use s3db::LocalBackend;
use std::collections::BTreeSet;

use gitquarry_federation::access;
use gitquarry_wire::Uid;

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&s3db::LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: s3db::LocalEngine::Files,
    });
    let cipher = Cipher::derive("test passphrase", &[13u8; s3db_crypto::SALT_LEN]).unwrap();
    (backend, cipher)
}

#[tokio::test]
async fn create_rejects_an_empty_repo_set() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let err = create(&backend, &cipher, "alice", "my token", BTreeSet::new(), false, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("at least one repo"));
}

#[tokio::test]
async fn create_then_revoke_roundtrips_and_revoke_is_owner_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let repos = BTreeSet::from([Uid::from("team/proj".to_string())]);
    let (token, secret) = create(&backend, &cipher, "alice", "my token", repos, false, None)
        .await
        .unwrap()
        .unwrap();
    assert!(secret.starts_with("token_"));

    let resolved = access::resolve_by_token(&backend, &cipher, &secret).await.unwrap();
    assert!(resolved.is_some());

    // A different owner can't revoke it.
    revoke(&backend, &cipher, "bob", token.uid.as_str()).await.unwrap();
    assert!(
        access::resolve_by_token(&backend, &cipher, &secret)
            .await
            .unwrap()
            .is_some()
    );

    revoke(&backend, &cipher, "alice", token.uid.as_str()).await.unwrap();
    assert!(
        access::resolve_by_token(&backend, &cipher, &secret)
            .await
            .unwrap()
            .is_none()
    );
}
