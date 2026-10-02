use gitquarry_federation::domain::fork::*;
use gitquarry_test_support::{backend, cipher};

use gitquarry_federation::access;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::entities::PeerAccessLevel;
use gitquarry_wire::Uid;

#[tokio::test]
async fn request_fork_rejects_a_peer_that_isnt_linked() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    let core = CoreClient::new("http://127.0.0.1:1/_internal".to_string(), "secret".to_string());
    let dest_uid = Uid::new();
    let err = request_fork(
        backend,
        &cipher,
        &core,
        RequestFork {
            peer_name: "not-linked",
            source_repo_name: "acme/demo",
            dest_full_name: "alice/demo",
            dest_owner_kind: DestOwnerKind::User,
            dest_owner_uid: &dest_uid,
            description: None,
            acting_username: "alice",
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no such linked peer"));
}

#[tokio::test]
async fn request_fork_rejects_a_browse_only_peer() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    let peer = gitquarry_federation::entities::Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        linked_at: String::new(),
        access_level: PeerAccessLevel::BrowseOnly,
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    access::put_peer(backend, &cipher, &peer, None).await.unwrap();

    let core = CoreClient::new("http://127.0.0.1:1/_internal".to_string(), "secret".to_string());
    let dest_uid = Uid::new();
    let err = request_fork(
        backend,
        &cipher,
        &core,
        RequestFork {
            peer_name: "acme",
            source_repo_name: "acme/demo",
            dest_full_name: "alice/demo",
            dest_owner_kind: DestOwnerKind::User,
            dest_owner_uid: &dest_uid,
            description: None,
            acting_username: "alice",
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("access level too low"));
}

#[tokio::test]
async fn request_fork_rejects_a_banned_peer_even_at_full_access_level() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let backend = backend.as_ref();
    let cipher = cipher();
    let peer = gitquarry_federation::entities::Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        linked_at: String::new(),
        access_level: PeerAccessLevel::Full,
        banned: true,
        ban_reason: Some("spamming forks".to_string()),
        banned_at: None,
        banned_by: "alice".to_string(),
        ..Default::default()
    };
    access::put_peer(backend, &cipher, &peer, None).await.unwrap();

    let core = CoreClient::new("http://127.0.0.1:1/_internal".to_string(), "secret".to_string());
    let dest_uid = Uid::new();
    let err = request_fork(
        backend,
        &cipher,
        &core,
        RequestFork {
            peer_name: "acme",
            source_repo_name: "acme/demo",
            dest_full_name: "alice/demo",
            dest_owner_kind: DestOwnerKind::User,
            dest_owner_uid: &dest_uid,
            description: None,
            acting_username: "alice",
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("is banned"));
    assert!(err.to_string().contains("spamming forks"));
}
