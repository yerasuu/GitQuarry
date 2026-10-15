use gitquarry_federation::access;
use gitquarry_federation::entities::{Peer, PeerAccessLevel};
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

    // Not requested locally yet: handshake doesn't confirm, and never even
    // dials the peer.
    assert!(
        !receive_handshake(backend, &cipher, "me", "acme", "shared-secret")
            .await
            .unwrap()
    );
    let peer = access::get_peer(backend, &cipher, "acme").await.unwrap().unwrap();
    assert!(!peer.sister_confirmed);
}

#[tokio::test]
async fn a_staged_request_secret_is_vouched_for_but_authenticates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", false).await;
    let staged = request(backend, &cipher, "acme").await.unwrap();

    assert!(verify_staged_secret(backend, &cipher, "acme", &staged).await.unwrap());
    assert!(!verify_staged_secret(backend, &cipher, "acme", "guessed").await.unwrap());
    assert!(!verify_staged_secret(backend, &cipher, "other", &staged).await.unwrap());
    assert!(sister_by_secret(backend, &cipher, &staged).await.unwrap().is_none());
}

#[tokio::test]
async fn receive_handshake_refuses_an_invalid_secret() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", false).await;
    request(backend, &cipher, "acme").await.unwrap();
    assert!(receive_handshake(backend, &cipher, "me", "acme", "").await.is_err());
    assert!(
        receive_handshake(backend, &cipher, "me", "acme", &"x".repeat(4096))
            .await
            .is_err()
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
    confirm_after_handshake(backend, &cipher, "acme", "secret-a")
        .await
        .unwrap();
    request(backend, &cipher, "other").await.unwrap();

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
    assert_eq!(peer.access_level, PeerAccessLevel::BrowseOnly);
}

fn event(peer_name: &str, git_base_url: &str, action: BanAction) -> BanEventWire {
    BanEventWire {
        peer_name: peer_name.to_string(),
        git_base_url: git_base_url.to_string(),
        action,
        reason: Some("abuse".to_string()),
        banned_by: "bob".to_string(),
        banned_at: None,
    }
}

#[tokio::test]
async fn apply_ban_event_refuses_an_invalid_name_or_url() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    for bad in [
        event("../escape", "https://x.example", BanAction::Ban),
        event("stranger", "file:///etc", BanAction::Ban),
        event("stranger", "--upload-pack=sh", BanAction::Ban),
    ] {
        assert!(apply_ban_event(backend, &cipher, "sis", &bad).await.is_err());
    }
    assert!(access::list_all_peers(backend, &cipher).await.unwrap().is_empty());
}

#[tokio::test]
async fn apply_ban_event_unban_of_an_unknown_name_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    apply_ban_event(
        backend,
        &cipher,
        "sis",
        &event("stranger", "https://stranger.example", BanAction::Unban),
    )
    .await
    .unwrap();
    assert!(access::get_peer(backend, &cipher, "stranger").await.unwrap().is_none());
}

#[tokio::test]
async fn a_sister_unban_never_lifts_a_ban_it_did_not_put_here() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", false).await;
    gitquarry_federation::domain::peer::ban(backend, &cipher, "acme", "local decision", "admin")
        .await
        .unwrap();
    seed_peer(backend, &cipher, "other", false).await;
    apply_ban_event(
        backend,
        &cipher,
        "sis-b",
        &event("other", "https://other.example", BanAction::Ban),
    )
    .await
    .unwrap();

    for (name, url) in [("acme", "https://acme.example"), ("other", "https://other.example")] {
        apply_ban_event(backend, &cipher, "sis-a", &event(name, url, BanAction::Unban))
            .await
            .unwrap();
        assert!(access::get_peer(backend, &cipher, name).await.unwrap().unwrap().banned);
    }
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
    assert_eq!(peer.access_level, PeerAccessLevel::BrowseOnly);
}

#[tokio::test]
async fn a_local_ban_echoed_back_by_a_sister_stays_this_instances_own() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme", false).await;
    gitquarry_federation::domain::peer::ban(backend, &cipher, "acme", "local decision", "admin")
        .await
        .unwrap();

    apply_ban_event(
        backend,
        &cipher,
        "sis",
        &event("acme", "https://acme.example", BanAction::Ban),
    )
    .await
    .unwrap();

    let peer = access::get_peer(backend, &cipher, "acme").await.unwrap().unwrap();
    assert_eq!(peer.synced_from_sister, None);
    assert_eq!(peer.ban_reason.as_deref(), Some("local decision"));
    assert_eq!(peer.banned_by, "admin");
}
