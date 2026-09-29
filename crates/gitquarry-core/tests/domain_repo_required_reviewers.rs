//! `domain::repo::set_required_reviewers`.

use gitquarry_core::domain::repo;

#[tokio::test]
async fn set_required_reviewers_rejects_an_unknown_username() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let resolved = repo::create(
        &ctx,
        "team/proj",
        None,
        None,
        None,
        gitquarry_core::storage::entities::Visibility::Public,
    )
    .await
    .unwrap()
    .unwrap();

    assert!(
        repo::set_required_reviewers(&ctx, &resolved, "nobody", gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
    let (config, _etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(config.required_reviewers.is_empty());
}

#[tokio::test]
async fn set_required_reviewers_stores_every_real_username() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let resolved = repo::create(
        &ctx,
        "team/proj",
        None,
        None,
        None,
        gitquarry_core::storage::entities::Visibility::Public,
    )
    .await
    .unwrap()
    .unwrap();
    for username in ["alice", "bob"] {
        gitquarry_test_support::user(&ctx, username, false).await;
    }

    let (cas, reviewers) = repo::set_required_reviewers(&ctx, &resolved, "alice, bob", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(cas, gitquarry_core::storage::Cas::Ok);
    assert_eq!(reviewers.len(), 2);
    let (config, _etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(config.required_reviewers.contains("alice"));
    assert!(config.required_reviewers.contains("bob"));
}
