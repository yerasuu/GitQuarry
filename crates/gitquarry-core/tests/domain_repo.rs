use gitquarry_core::app::ResolvedRepo;
use gitquarry_core::domain::repo::*;
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{self, Cas};
use std::sync::Arc;

fn test_resolved(root: &std::path::Path) -> ResolvedRepo {
    let remote = Remote::new(
        gitquarry_test_support::backend(root),
        "repos/team/proj".to_string(),
        gitquarry_test_support::cipher(),
    );
    ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
    }
}

#[tokio::test]
async fn create_tag_writes_the_ref_into_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    assert_eq!(
        create_tag(&resolved, "v1.0", &"a".repeat(40), gitquarry_i18n::Locale::Es)
            .await
            .unwrap(),
        Cas::Ok
    );
    let (manifest, _etag) = resolved.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.refs.get("refs/tags/v1.0"), Some(&"a".repeat(40)));
}

#[tokio::test]
async fn create_tag_rejects_a_name_that_already_names_a_ref() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/tags/v1.0".to_string(), "a".repeat(40));
    resolved.remote.put_manifest(&manifest, None).await.unwrap();

    assert!(
        create_tag(&resolved, "v1.0", &"b".repeat(40), gitquarry_i18n::Locale::Es)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn delete_tag_removes_the_ref_from_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/tags/v1.0".to_string(), "a".repeat(40));
    resolved.remote.put_manifest(&manifest, None).await.unwrap();

    assert_eq!(
        delete_tag(&resolved, "v1.0", gitquarry_i18n::Locale::Es).await.unwrap(),
        Cas::Ok
    );
    let (manifest, _etag) = resolved.remote.get_manifest().await.unwrap();
    assert!(!manifest.refs.contains_key("refs/tags/v1.0"));
}

#[tokio::test]
async fn delete_tag_rejects_a_nonexistent_tag() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    assert!(delete_tag(&resolved, "v1.0", gitquarry_i18n::Locale::Es).await.is_err());
}

#[tokio::test]
async fn delete_tag_rejects_a_protected_tag_even_though_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let backend = gitquarry_test_support::backend(dir.path());
    let cipher = gitquarry_test_support::cipher();
    let remote = Remote::new(Arc::clone(&backend), "repos/team/proj".to_string(), Arc::clone(&cipher));
    let resolved = ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
    };

    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/tags/v1.0".to_string(), "a".repeat(40));
    resolved.remote.put_manifest(&manifest, None).await.unwrap();

    let mut config = gitquarry_core::storage::entities::RepoConfig::default();
    config.protected_tags.insert("v1.0".to_string());
    storage::settings::put_repo_config(backend.as_ref(), "team/proj", &config, None, &cipher)
        .await
        .unwrap();

    assert!(delete_tag(&resolved, "v1.0", gitquarry_i18n::Locale::Es).await.is_err());
    let (manifest, _etag) = resolved.remote.get_manifest().await.unwrap();
    assert!(manifest.refs.contains_key("refs/tags/v1.0"));
}

#[tokio::test]
async fn delete_branch_removes_the_ref_from_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/feature".to_string(), "a".repeat(40));
    manifest.head = Some("refs/heads/main".to_string());
    resolved.remote.put_manifest(&manifest, None).await.unwrap();

    assert_eq!(
        delete_branch(&resolved, "feature", gitquarry_i18n::Locale::Es)
            .await
            .unwrap(),
        Cas::Ok
    );
    let (manifest, _etag) = resolved.remote.get_manifest().await.unwrap();
    assert!(!manifest.refs.contains_key("refs/heads/feature"));
}

#[tokio::test]
async fn delete_branch_rejects_a_nonexistent_branch() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    assert!(
        delete_branch(&resolved, "feature", gitquarry_i18n::Locale::Es)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn delete_branch_rejects_the_default_branch() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), "a".repeat(40));
    manifest.head = Some("refs/heads/main".to_string());
    resolved.remote.put_manifest(&manifest, None).await.unwrap();

    assert!(
        delete_branch(&resolved, "main", gitquarry_i18n::Locale::Es)
            .await
            .is_err()
    );
    let (manifest, _etag) = resolved.remote.get_manifest().await.unwrap();
    assert!(manifest.refs.contains_key("refs/heads/main"));
}

#[tokio::test]
async fn delete_branch_rejects_a_protected_branch_even_though_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let backend = gitquarry_test_support::backend(dir.path());
    let cipher = gitquarry_test_support::cipher();
    let remote = Remote::new(Arc::clone(&backend), "repos/team/proj".to_string(), Arc::clone(&cipher));
    let resolved = ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
    };

    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/release".to_string(), "a".repeat(40));
    manifest.head = Some("refs/heads/main".to_string());
    resolved.remote.put_manifest(&manifest, None).await.unwrap();

    let mut config = gitquarry_core::storage::entities::RepoConfig::default();
    config.protected_branches.insert("release".to_string());
    storage::settings::put_repo_config(backend.as_ref(), "team/proj", &config, None, &cipher)
        .await
        .unwrap();

    assert!(
        delete_branch(&resolved, "release", gitquarry_i18n::Locale::Es)
            .await
            .is_err()
    );
    let (manifest, _etag) = resolved.remote.get_manifest().await.unwrap();
    assert!(manifest.refs.contains_key("refs/heads/release"));
}

#[test]
fn slugify_repo_name_replaces_unsafe_characters_and_preserves_case() {
    assert_eq!(slugify_repo_name("My Cool Repo!").as_deref(), Some("My-Cool-Repo"));
    assert_eq!(slugify_repo_name("  weird///na@me  ").as_deref(), Some("weird-na-me"));
    assert_eq!(
        slugify_repo_name("already-clean_v1.0").as_deref(),
        Some("already-clean_v1.0")
    );
}

#[test]
fn slugify_repo_name_drops_names_with_nothing_safe_to_keep() {
    assert_eq!(slugify_repo_name("   "), None);
    assert_eq!(slugify_repo_name("!!!"), None);
    assert_eq!(slugify_repo_name("日本語"), None);
}

#[test]
fn slugify_repo_name_caps_length_and_retrims() {
    let long = "a".repeat(150);
    let slug = slugify_repo_name(&long).unwrap();
    assert_eq!(slug.len(), 100);

    let long_with_trailing_dash = format!("{}-{}", "a".repeat(99), "b".repeat(50));
    let slug = slugify_repo_name(&long_with_trailing_dash).unwrap();
    assert!(!slug.ends_with('-'));
}

#[test]
fn parse_csv_field_splits_and_trims() {
    let branches = parse_csv_field("main, release/1.0 ,  ,develop");
    assert_eq!(branches.len(), 3);
    assert!(branches.contains("main"));
    assert!(branches.contains("release/1.0"));
    assert!(branches.contains("develop"));
}

#[tokio::test]
async fn set_topics_normalizes_each_entry_and_drops_the_ones_left_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let repo = ctx.create_repo("team/proj").await.unwrap().unwrap();
    let long = "a".repeat(50);

    let (cas, topics) = set_topics(&ctx, &repo, &format!(" Rust! ,git-cli,   ,!!!,{long}"))
        .await
        .unwrap();
    assert_eq!(cas, Cas::Ok);
    let expected: std::collections::BTreeSet<String> = ["rust", "git-cli", &"a".repeat(32)]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(topics, expected);
    assert_eq!(repo.remote.get_repo_config().await.unwrap().0.topics, expected);
}

#[tokio::test]
async fn set_labels_stores_name_color_pairs_last_duplicate_winning() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let repo = ctx.create_repo("team/proj").await.unwrap().unwrap();

    let (_, labels) = set_labels(&ctx, &repo, "bug:#111111, enhancement:#a2eeef, bug:#d73a4a")
        .await
        .unwrap();
    let stored: Vec<(String, String)> = repo
        .remote
        .get_repo_config()
        .await
        .unwrap()
        .0
        .labels
        .into_iter()
        .map(|l| (l.name, l.color))
        .collect();
    assert_eq!(labels.len(), 2);
    assert_eq!(
        stored,
        [("bug", "#d73a4a"), ("enhancement", "#a2eeef")].map(|(n, c)| (n.to_string(), c.to_string()))
    );
}

#[tokio::test]
async fn set_labels_rejects_a_malformed_entry_and_keeps_the_stored_labels() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let repo = ctx.create_repo("team/proj").await.unwrap().unwrap();
    set_labels(&ctx, &repo, "bug:#d73a4a").await.unwrap();

    for bad in ["bug:red", "bug:#zzzzzz", ":#d73a4a", "no-color"] {
        assert!(set_labels(&ctx, &repo, bad).await.is_err(), "{bad} was accepted");
    }
    assert_eq!(repo.remote.get_repo_config().await.unwrap().0.labels.len(), 1);
}
