use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::entities::OwnerRef;
use gitquarry_core::storage::settings::*;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::{backend, cipher};

/// A claim that must land as a plain create: `still_live` panics, so any
/// conflict path (a name that wasn't actually free) fails the test.
async fn claim_fresh(backend: &dyn Backend, cipher: &Cipher, full_name: &str, uid: &Uid) -> bool {
    claim_repo_name_self_healing(backend, cipher, full_name, uid, |_| async move {
        panic!("{full_name} was expected to be free")
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn resolve_repo_name_is_none_before_any_claim() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    assert_eq!(
        resolve_repo_name(backend.as_ref(), &cipher, "acme/proj").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn self_healing_claim_succeeds_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let uid = Uid::new();
    let claimed = claim_repo_name_self_healing(backend.as_ref(), &cipher, "acme/proj", &uid, |_| async { Ok(true) })
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(
        resolve_repo_name(backend.as_ref(), &cipher, "acme/proj").await.unwrap(),
        Some(uid)
    );
}

#[tokio::test]
async fn self_healing_claim_refuses_a_genuinely_live_owner() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let first = Uid::new();
    let second = Uid::new();
    assert!(claim_fresh(backend.as_ref(), &cipher, "acme/proj", &first).await);
    let claimed = claim_repo_name_self_healing(backend.as_ref(), &cipher, "acme/proj", &second, |_| async { Ok(true) })
        .await
        .unwrap();
    assert!(!claimed);
    assert_eq!(
        resolve_repo_name(backend.as_ref(), &cipher, "acme/proj").await.unwrap(),
        Some(first)
    );
}

#[tokio::test]
async fn self_healing_claim_reclaims_a_stale_owner() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let stale = Uid::new();
    let fresh = Uid::new();
    assert!(claim_fresh(backend.as_ref(), &cipher, "acme/proj", &stale).await);
    let claimed = claim_repo_name_self_healing(backend.as_ref(), &cipher, "acme/proj", &fresh, |_| async { Ok(false) })
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(
        resolve_repo_name(backend.as_ref(), &cipher, "acme/proj").await.unwrap(),
        Some(fresh)
    );
}

#[tokio::test]
async fn owner_index_lists_only_that_owners_repos() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let owner = OwnerRef::User(Uid::new());
    let other_owner = OwnerRef::User(Uid::new());
    let mine = Uid::new();
    let not_mine = Uid::new();
    add_owner_index(backend.as_ref(), &cipher, &owner, &mine).await;
    add_owner_index(backend.as_ref(), &cipher, &other_owner, &not_mine).await;
    let uids = list_owned_repo_uids(backend.as_ref(), &owner).await.unwrap();
    assert_eq!(uids, vec![mine]);
}

#[tokio::test]
async fn owner_index_ignores_an_owner_with_an_empty_uid() {
    // A `RepoConfig::owner` in this shape predates ever having a real
    // uid (or is otherwise corrupt) -- indexing it would build a key
    // with an empty path segment, so both the write and the read side
    // treat it as "nothing to index" instead.
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let unset_owner = OwnerRef::User(Uid::default());
    let repo_uid = Uid::new();
    add_owner_index(backend.as_ref(), &cipher, &unset_owner, &repo_uid).await;
    assert!(
        list_owned_repo_uids(backend.as_ref(), &unset_owner)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn owner_index_forgets_a_removed_repo() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let owner = OwnerRef::User(Uid::new());
    let repo_uid = Uid::new();
    add_owner_index(backend.as_ref(), &cipher, &owner, &repo_uid).await;
    remove_owner_index(backend.as_ref(), &cipher, &owner, &repo_uid).await;
    assert!(list_owned_repo_uids(backend.as_ref(), &owner).await.unwrap().is_empty());
}

#[tokio::test]
async fn bump_pack_bytes_and_lfs_bytes_accumulate_independently() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let repo_uid = Uid::new();
    put_repo_config(
        backend.as_ref(),
        repo_uid.as_str(),
        &gitquarry_core::storage::entities::RepoConfig {
            uid: repo_uid.clone(),
            ..Default::default()
        },
        None,
        &cipher,
    )
    .await
    .unwrap();

    bump_pack_bytes(backend.as_ref(), &cipher, &repo_uid, 100).await;
    bump_pack_bytes(backend.as_ref(), &cipher, &repo_uid, 50).await;
    bump_lfs_bytes(backend.as_ref(), &cipher, &repo_uid, 10).await;

    let (config, _etag) = get_repo_config(backend.as_ref(), repo_uid.as_str(), &cipher)
        .await
        .unwrap();
    assert_eq!(config.pack_bytes, 150);
    assert_eq!(config.lfs_bytes, 10);
}

#[tokio::test]
async fn bump_lfs_bytes_counts_through_a_cache_another_process_made_stale() {
    let dir = tempfile::tempdir().unwrap();
    let raw = backend(dir.path());
    let cached = s3db::CachedBackend::new(std::sync::Arc::clone(&raw), s3db::CacheConfig::default());
    let cipher = cipher();
    let repo_uid = Uid::new();
    put_repo_config(
        &cached,
        repo_uid.as_str(),
        &gitquarry_core::storage::entities::RepoConfig {
            uid: repo_uid.clone(),
            ..Default::default()
        },
        None,
        &cipher,
    )
    .await
    .unwrap();
    // The push hook runs out of process: its write bypasses the server's cache.
    bump_pack_bytes(raw.as_ref(), &cipher, &repo_uid, 100).await;

    bump_lfs_bytes(&cached, &cipher, &repo_uid, 10).await;

    let (config, _etag) = get_repo_config(raw.as_ref(), repo_uid.as_str(), &cipher).await.unwrap();
    assert_eq!(config.pack_bytes, 100);
    assert_eq!(config.lfs_bytes, 10);
}

#[tokio::test]
async fn bump_bytes_is_a_silent_no_op_for_a_repo_that_doesnt_exist() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    // No panic, no error surfaced -- just nothing to update.
    bump_pack_bytes(backend.as_ref(), &cipher, &Uid::new(), 100).await;
}

#[tokio::test]
async fn sum_owner_repo_bytes_adds_pack_and_lfs_across_every_owned_repo() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let owner = OwnerRef::User(Uid::new());
    let other_owner = OwnerRef::User(Uid::new());

    for (uid, pack, lfs, owner_ref) in [
        (Uid::new(), 100u64, 20u64, &owner),
        (Uid::new(), 30, 5, &owner),
        (Uid::new(), 1_000, 1_000, &other_owner),
    ] {
        put_repo_config(
            backend.as_ref(),
            uid.as_str(),
            &gitquarry_core::storage::entities::RepoConfig {
                uid: uid.clone(),
                owner: Some(owner_ref.clone()),
                pack_bytes: pack,
                lfs_bytes: lfs,
                ..Default::default()
            },
            None,
            &cipher,
        )
        .await
        .unwrap();
        add_owner_index(backend.as_ref(), &cipher, owner_ref, &uid).await;
    }

    assert_eq!(
        sum_owner_repo_bytes(backend.as_ref(), &cipher, &owner).await.unwrap(),
        155
    );
    assert_eq!(
        sum_owner_repo_bytes(backend.as_ref(), &cipher, &other_owner)
            .await
            .unwrap(),
        2_000
    );
}

#[tokio::test]
async fn visible_index_roundtrips_and_clears() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let repo_uid = Uid::new();
    set_visible_index(backend.as_ref(), &cipher, &repo_uid, true).await;
    assert_eq!(
        list_visible_repo_uids(backend.as_ref()).await.unwrap(),
        vec![repo_uid.clone()]
    );
    set_visible_index(backend.as_ref(), &cipher, &repo_uid, false).await;
    assert!(list_visible_repo_uids(backend.as_ref()).await.unwrap().is_empty());
}

#[tokio::test]
async fn mirrors_enabled_index_tracks_set_and_clear_independently_per_repo() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    let a = Uid::from("team/a".to_string());
    let b = Uid::from("team/b".to_string());

    set_mirrors_enabled_index(backend.as_ref(), &cipher, &a, true)
        .await
        .unwrap();
    set_mirrors_enabled_index(backend.as_ref(), &cipher, &b, true)
        .await
        .unwrap();
    assert!(has_enabled_mirror(backend.as_ref(), &cipher, &a).await);
    let mut listed = list_repos_with_enabled_mirrors(backend.as_ref()).await.unwrap();
    listed.sort();
    assert_eq!(listed, vec![a.clone(), b.clone()]);

    set_mirrors_enabled_index(backend.as_ref(), &cipher, &a, false)
        .await
        .unwrap();
    assert!(!has_enabled_mirror(backend.as_ref(), &cipher, &a).await);
    let listed = list_repos_with_enabled_mirrors(backend.as_ref()).await.unwrap();
    assert_eq!(listed, vec![b]);
}

#[tokio::test]
async fn release_then_claim_succeeds_under_a_fresh_etag() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path());
    let cipher = cipher();
    assert!(claim_fresh(backend.as_ref(), &cipher, "acme/proj", &Uid::new()).await);
    release_repo_name(backend.as_ref(), "acme/proj").await;
    assert_eq!(
        resolve_repo_name(backend.as_ref(), &cipher, "acme/proj").await.unwrap(),
        None
    );
    let new_uid = Uid::new();
    assert!(claim_fresh(backend.as_ref(), &cipher, "acme/proj", &new_uid).await);
    assert_eq!(
        resolve_repo_name(backend.as_ref(), &cipher, "acme/proj").await.unwrap(),
        Some(new_uid)
    );
}
