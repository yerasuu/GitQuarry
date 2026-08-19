// `repo_index` itself is a private submodule of `auth::access` (see that
// module's doc comment); its public items are re-exported at
// `auth::access::*`, which is the path visible from outside the crate.
use gitquarry_core::auth::access::*;
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;

fn user_with_repos(username: &str, repos: &[(&str, Permission)]) -> User {
    User {
        uid: Uid::new(),
        username: username.to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: repos.iter().map(|(r, p)| (Uid::from(r.to_string()), *p)).collect(),
        default_permission: None,
        password_hash: None,
        is_admin: false,
        session_epoch: 0,
        is_federation_moderator: false,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    }
}

fn test_backend_and_cipher(root: &std::path::Path) -> (gitquarry_core::storage::local::LocalBackend, Cipher) {
    let backend = gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: gitquarry_core::config::LocalEngine::Files,
    });
    let cipher = Cipher::derive("test passphrase", &[9u8; gitquarry_core::crypto::SALT_LEN]).unwrap();
    (backend, cipher)
}

#[tokio::test]
async fn repo_permission_index_lists_only_the_granted_user() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let repo_uid = Uid::new();

    let mut alice = user_with_repos("alice", &[]);
    alice.repos.insert(repo_uid.clone(), Permission::Developer);
    put_user(backend.as_ref(), &cipher, &alice, None).await.unwrap();
    let _ = alice.claim_uid(backend.as_ref(), &cipher).await;
    add_repo_permission_index(backend.as_ref(), &cipher, &repo_uid, &alice.uid).await;

    let bob = user_with_repos("bob", &[]); // no permission on repo_uid at all
    put_user(backend.as_ref(), &cipher, &bob, None).await.unwrap();
    let _ = bob.claim_uid(backend.as_ref(), &cipher).await;

    let granted = list_repo_permissions(&backend, &cipher, &repo_uid).await.unwrap();
    assert_eq!(granted.len(), 1);
    assert_eq!(granted[0].0.username, "alice");
    assert_eq!(granted[0].1, Permission::Developer);
}

#[tokio::test]
async fn removing_a_permission_drops_it_from_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let repo_uid = Uid::new();
    let alice = user_with_repos("alice", &[]);
    add_repo_permission_index(backend.as_ref(), &cipher, &repo_uid, &alice.uid).await;
    remove_repo_permission_index(backend.as_ref(), &cipher, &repo_uid, &alice.uid).await;
    assert!(
        list_repo_permissions(&backend, &cipher, &repo_uid)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn clear_repo_permission_index_drops_every_entry_for_that_repo() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let backend: Arc<dyn Backend> = Arc::new(backend);
    let cipher = Arc::new(cipher);
    let repo_uid = Uid::new();
    let alice = user_with_repos("alice", &[]);
    let bob = user_with_repos("bob", &[]);
    add_repo_permission_index(backend.as_ref(), &cipher, &repo_uid, &alice.uid).await;
    add_repo_permission_index(backend.as_ref(), &cipher, &repo_uid, &bob.uid).await;
    clear_repo_permission_index(backend.as_ref(), &cipher, &repo_uid)
        .await
        .unwrap();
    assert!(
        list_repo_permissions(&backend, &cipher, &repo_uid)
            .await
            .unwrap()
            .is_empty()
    );
}
