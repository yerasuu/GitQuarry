use gitquarry_federation::access;
use gitquarry_federation::domain::peer::*;
use gitquarry_federation::entities::{Peer, PeerAccessLevel};
use gitquarry_test_support::{backend, cipher};
use s3db::{Backend, Cipher};

#[tokio::test]
async fn add_rejects_a_peer_thats_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    let err = add(
        backend,
        &cipher,
        "dead-peer",
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        PeerAccessLevel::Full,
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("couldn't reach"));
}

#[tokio::test]
async fn add_rejects_an_invalid_name() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    assert!(
        add(
            backend,
            &cipher,
            "bad/name",
            "https://acme.example",
            "https://acme.example",
            PeerAccessLevel::Full,
            false,
        )
        .await
        .is_err()
    );
}

async fn seed_peer(backend: &dyn Backend, cipher: &Cipher, name: &str) {
    let peer = Peer {
        name: name.to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        access_level: PeerAccessLevel::Full,
        ..Default::default()
    };
    access::put_peer(backend, cipher, &peer, None).await.unwrap();
}

#[tokio::test]
async fn ban_rejects_an_empty_reason() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme").await;
    assert!(ban(backend, &cipher, "acme", "   ", "alice").await.is_err());
}

#[tokio::test]
async fn ban_is_a_no_op_success_for_an_unknown_peer() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    assert!(!ban(backend, &cipher, "nobody", "spam", "alice").await.unwrap());
}

#[tokio::test]
async fn ban_then_unban_roundtrips_and_unban_resets_to_browse_only() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme").await;

    assert!(ban(backend, &cipher, "acme", "spamming", "alice").await.unwrap());
    let peer = access::get_peer(backend, &cipher, "acme").await.unwrap().unwrap();
    assert!(peer.banned);
    assert_eq!(peer.ban_reason.as_deref(), Some("spamming"));
    assert_eq!(peer.banned_by, "alice");

    assert!(unban(backend, &cipher, "acme").await.unwrap());
    let peer = access::get_peer(backend, &cipher, "acme").await.unwrap().unwrap();
    assert!(!peer.banned);
    assert_eq!(peer.ban_reason, None);
    assert_eq!(peer.access_level, PeerAccessLevel::BrowseOnly);
}

#[tokio::test]
async fn unban_is_a_no_op_for_a_peer_thats_not_banned() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme").await;
    assert!(!unban(backend, &cipher, "acme").await.unwrap());
}

#[tokio::test]
async fn add_refuses_to_re_add_a_banned_peer_without_an_explicit_unban() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    seed_peer(backend, &cipher, "acme").await;
    ban(backend, &cipher, "acme", "abuse", "alice").await.unwrap();

    let err = add(
        backend,
        &cipher,
        "acme",
        "https://acme.example",
        "https://acme.example",
        PeerAccessLevel::Full,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("previously banned"));
    assert!(err.to_string().contains("abuse"));
}
