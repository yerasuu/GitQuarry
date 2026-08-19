use gitquarry_federation::access;
use gitquarry_federation::domain::peer_credential::*;
use gitquarry_federation::entities::Peer;
use gitquarry_wire::Uid;
use s3db::{Backend, Cipher, LocalBackend};

async fn seed_peer(backend: &dyn Backend, cipher: &Cipher, name: &str) {
    let peer = Peer {
        name: name.to_string(),
        web_base_url: format!("https://{name}.example"),
        git_base_url: format!("https://{name}.example"),
        linked_at: "2026-08-05T00:00:00Z".to_string(),
        access_level: Default::default(),
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    access::put_peer(backend, cipher, &peer, None).await.unwrap();
}

fn test_backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&s3db::LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: s3db::LocalEngine::Files,
    });
    let cipher = Cipher::derive("test passphrase", &[21u8; s3db_crypto::SALT_LEN]).unwrap();
    (backend, cipher)
}

#[tokio::test]
async fn set_rejects_a_peer_name_that_isnt_actually_linked() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let err = set(
        &backend,
        &cipher,
        &Uid::new(),
        "not-a-real-peer",
        "token_abc",
        "my label",
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no such linked peer"));
}

#[tokio::test]
async fn set_then_get_roundtrips_and_a_second_set_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    seed_peer(&backend, &cipher, "acme").await;
    let user_uid = Uid::new();

    set(&backend, &cipher, &user_uid, "acme", "token_first", "first label")
        .await
        .unwrap();
    let credential = get(&backend, &cipher, &user_uid, "acme").await.unwrap().unwrap();
    assert_eq!(credential.secret, "token_first");

    set(&backend, &cipher, &user_uid, "acme", "token_second", "second label")
        .await
        .unwrap();
    let credential = get(&backend, &cipher, &user_uid, "acme").await.unwrap().unwrap();
    assert_eq!(credential.secret, "token_second");
    assert_eq!(credential.label, "second label");
}

#[tokio::test]
async fn list_and_delete_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    seed_peer(&backend, &cipher, "acme").await;
    seed_peer(&backend, &cipher, "other").await;
    let user_uid = Uid::new();

    set(&backend, &cipher, &user_uid, "acme", "token_a", "a").await.unwrap();
    set(&backend, &cipher, &user_uid, "other", "token_b", "b")
        .await
        .unwrap();
    let listed = list(&backend, &cipher, &user_uid).await.unwrap();
    assert_eq!(
        listed.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        vec!["acme", "other"]
    );

    delete(&backend, &cipher, &user_uid, "acme").await;
    assert!(get(&backend, &cipher, &user_uid, "acme").await.unwrap().is_none());
    assert!(get(&backend, &cipher, &user_uid, "other").await.unwrap().is_some());
}
