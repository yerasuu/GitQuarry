use gitquarry_core::app::ResolvedRepo;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::domain::repo::*;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::uid::Uid;
use gitquarry_core::storage::{self, Cas};
use std::sync::Arc;

fn test_resolved(root: &std::path::Path) -> ResolvedRepo {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; SALT_LEN]).unwrap());
    let remote = Remote::new(backend, "repos/team/proj".to_string(), cipher);
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
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: dir.path().to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; SALT_LEN]).unwrap());
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
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: dir.path().to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; SALT_LEN]).unwrap());
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
fn normalize_topic_lowercases_and_strips_punctuation() {
    assert_eq!(normalize_topic(" Rust! ").as_deref(), Some("rust"));
    assert_eq!(normalize_topic("git-cli").as_deref(), Some("git-cli"));
}

#[test]
fn normalize_topic_drops_tokens_that_become_empty() {
    assert_eq!(normalize_topic("   "), None);
    assert_eq!(normalize_topic("!!!"), None);
}

#[test]
fn normalize_topic_caps_length() {
    let long = "a".repeat(50);
    assert_eq!(normalize_topic(&long).unwrap().len(), 32);
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
fn parse_labels_csv_parses_name_color_pairs() {
    let labels = parse_labels_csv("bug:#d73a4a, enhancement:#a2eeef").unwrap();
    assert_eq!(labels.len(), 2);
    assert_eq!(labels[0].name, "bug");
    assert_eq!(labels[0].color, "#d73a4a");
    assert_eq!(labels[1].name, "enhancement");
}

#[test]
fn parse_labels_csv_rejects_a_malformed_color() {
    assert!(parse_labels_csv("bug:red").is_err());
    assert!(parse_labels_csv("bug:#zzzzzz").is_err());
}

#[test]
fn parse_labels_csv_rejects_an_empty_name() {
    assert!(parse_labels_csv(":#d73a4a").is_err());
}

#[test]
fn parse_labels_csv_last_entry_wins_on_duplicate_name() {
    let labels = parse_labels_csv("bug:#111111, bug:#222222").unwrap();
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].color, "#222222");
}

#[test]
fn parse_csv_field_splits_and_trims() {
    let branches = parse_csv_field("main, release/1.0 ,  ,develop");
    assert_eq!(branches.len(), 3);
    assert!(branches.contains("main"));
    assert!(branches.contains("release/1.0"));
    assert!(branches.contains("develop"));
}
