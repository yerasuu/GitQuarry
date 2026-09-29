use gitquarry_federation::access;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::sister::*;
use gitquarry_test_support::{backend, cipher};
use s3db::{Backend, Cipher};

async fn seed_peer(backend: &dyn Backend, cipher: &Cipher, name: &str, auto_linked: bool) {
    let peer = Peer {
        name: name.to_string(),
        web_base_url: format!("https://{name}.example"),
        git_base_url: format!("https://{name}.example"),
        auto_linked,
        ..Default::default()
    };
    access::put_peer(backend, cipher, &peer, None).await.unwrap();
}

#[tokio::test]
async fn request_rejects_an_auto_linked_peer() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", true).await;
    let err = request(backend, &cipher, "acme").await.unwrap_err();
    assert!(err.to_string().contains("auto-linked"));
}

#[tokio::test]
async fn request_rejects_an_unknown_peer() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    assert!(request(backend, &cipher, "nobody").await.is_err());
}

#[tokio::test]
async fn receive_handshake_requires_a_matching_local_request_first() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", false).await;

    // Not requested locally yet: handshake doesn't confirm.
    assert!(
        !receive_handshake(backend, &cipher, "acme", "shared-secret")
            .await
            .unwrap()
    );

    request(backend, &cipher, "acme").await.unwrap();
    assert!(
        receive_handshake(backend, &cipher, "acme", "shared-secret")
            .await
            .unwrap()
    );

    let peer = access::get_peer(backend, &cipher, "acme").await.unwrap().unwrap();
    assert!(peer.sister_confirmed);
    assert_eq!(
        credential_for(backend, &cipher, "acme").await.unwrap().as_deref(),
        Some("shared-secret")
    );
}

#[tokio::test]
async fn sister_by_secret_finds_the_right_peer_and_ignores_unconfirmed_ones() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", false).await;
    seed_peer(backend, &cipher, "other", false).await;
    request(backend, &cipher, "acme").await.unwrap();
    receive_handshake(backend, &cipher, "acme", "secret-a").await.unwrap();

    let found = sister_by_secret(backend, &cipher, "secret-a").await.unwrap();
    assert_eq!(found.map(|p| p.name), Some("acme".to_string()));
    assert!(
        sister_by_secret(backend, &cipher, "wrong-secret")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn apply_ban_event_creates_a_stub_peer_for_an_unknown_name() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    apply_ban_event(
        backend,
        &cipher,
        "sister-instance",
        &BanEventWire {
            peer_name: "stranger".to_string(),
            git_base_url: "https://stranger.example".to_string(),
            action: BanAction::Ban,
            reason: Some("abuse".to_string()),
            banned_by: "bob".to_string(),
            banned_at: None,
        },
    )
    .await
    .unwrap();

    let peer = access::get_peer(backend, &cipher, "stranger").await.unwrap().unwrap();
    assert!(peer.banned);
    assert_eq!(peer.ban_reason.as_deref(), Some("abuse"));
    assert_eq!(peer.banned_by, "bob");
    assert_eq!(peer.synced_from_sister.as_deref(), Some("sister-instance"));
}

#[tokio::test]
async fn apply_ban_event_unban_clears_the_ban_and_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", false).await;
    apply_ban_event(
        backend,
        &cipher,
        "sister-instance",
        &BanEventWire {
            peer_name: "acme".to_string(),
            git_base_url: "https://acme.example".to_string(),
            action: BanAction::Ban,
            reason: Some("abuse".to_string()),
            banned_by: "bob".to_string(),
            banned_at: None,
        },
    )
    .await
    .unwrap();
    apply_ban_event(
        backend,
        &cipher,
        "sister-instance",
        &BanEventWire {
            peer_name: "acme".to_string(),
            git_base_url: "https://acme.example".to_string(),
            action: BanAction::Unban,
            reason: None,
            banned_by: String::new(),
            banned_at: None,
        },
    )
    .await
    .unwrap();

    let peer = access::get_peer(backend, &cipher, "acme").await.unwrap().unwrap();
    assert!(!peer.banned);
    assert_eq!(peer.synced_from_sister, None);
}
