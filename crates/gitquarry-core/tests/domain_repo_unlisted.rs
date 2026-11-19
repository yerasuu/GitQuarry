//! `Visibility::Unlisted` through the public domain and storage API: kept
//! out of `.conf/index/repo-visible` on every path that writes it, never
//! exposed to federation, and stored in a form older versions refuse to read
//! rather than misread as public.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::domain::repo::{self, ConfPatch};
use gitquarry_core::storage::entities::{OwnerRef, RepoConfig, Visibility};
use gitquarry_core::storage::settings;
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;

async fn listed(ctx: &AppContext, uid: &Uid) -> bool {
    settings::list_visible_repo_uids(ctx.backend().as_ref())
        .await
        .unwrap()
        .contains(uid)
}

async fn create(ctx: &AppContext, name: &str, visibility: Visibility) -> ResolvedRepo {
    repo::create(ctx, name, None, None, None, visibility)
        .await
        .unwrap()
        .unwrap()
}

async fn set_visibility(ctx: &AppContext, resolved: &ResolvedRepo, visibility: Visibility) {
    let patch = ConfPatch {
        description: None,
        owner: None,
        visibility,
        archived: false,
        is_template: false,
    };
    let (cas, _changed) = repo::set_conf(ctx, resolved, patch).await.unwrap();
    assert_eq!(cas, gitquarry_core::storage::Cas::Ok);
}

#[test]
fn unlisted_is_readable_by_link_but_never_listed() {
    assert!(Visibility::Unlisted.is_readable_by_link());
    assert!(!Visibility::Unlisted.is_listed());
    assert!(Visibility::Public.is_readable_by_link() && Visibility::Public.is_listed());
    assert!(!Visibility::Limited.is_readable_by_link() && Visibility::Limited.is_listed());
    assert!(!Visibility::Private.is_readable_by_link() && !Visibility::Private.is_listed());
}

#[test]
fn unlisted_serializes_lowercase() {
    assert_eq!(serde_json::to_string(&Visibility::Unlisted).unwrap(), r#""unlisted""#);
    let parsed: Visibility = serde_json::from_str(r#""unlisted""#).unwrap();
    assert_eq!(parsed, Visibility::Unlisted);
}

#[tokio::test]
async fn a_stored_unlisted_config_reads_back_unlisted() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let resolved = create(&ctx, "acme/hidden", Visibility::Unlisted).await;
    let (config, etag) = resolved.remote.get_repo_config().await.unwrap();
    assert!(etag.is_some());
    assert_eq!(config.visibility, Visibility::Unlisted);
    assert_eq!(config.full_name, "acme/hidden");
}

/// The tiers a version before `Unlisted` knows about, decoding the same
/// stored object the way that version's `RepoConfig` would.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // Only ever built by serde, which the lint can't see.
enum PreUnlistedVisibility {
    #[default]
    Public,
    Limited,
    Private,
}

#[derive(serde::Deserialize, Default)]
struct PreUnlistedConfig {
    #[serde(default)]
    visibility: PreUnlistedVisibility,
}

#[tokio::test]
async fn a_version_without_unlisted_fails_to_read_it_instead_of_defaulting_to_public() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let resolved = create(&ctx, "acme/hidden", Visibility::Unlisted).await;
    let key = format!("repos/{}/.conf/config.json", resolved.uid.as_str());

    let old = s3db::payload::get_json::<PreUnlistedConfig>(ctx.backend().as_ref(), &key, &ctx.cipher).await;
    let err = old.err().expect("an unknown visibility must not decode");
    assert!(format!("{err:#}").contains("unlisted"), "{err:#}");

    set_visibility(&ctx, &resolved, Visibility::Private).await;
    let old = s3db::payload::get_json::<PreUnlistedConfig>(ctx.backend().as_ref(), &key, &ctx.cipher).await;
    assert!(matches!(old.unwrap().0.visibility, PreUnlistedVisibility::Private));
}

#[tokio::test]
async fn creating_an_unlisted_repo_leaves_it_out_of_the_visible_index() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let hidden = create(&ctx, "acme/hidden", Visibility::Unlisted).await;
    let shown = create(&ctx, "acme/shown", Visibility::Public).await;
    assert!(!listed(&ctx, &hidden.uid).await);
    assert!(listed(&ctx, &shown.uid).await);
}

#[tokio::test]
async fn changing_visibility_updates_the_visible_index_each_way() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let resolved = create(&ctx, "acme/proj", Visibility::Public).await;
    assert!(listed(&ctx, &resolved.uid).await);

    for (visibility, expected) in [
        (Visibility::Unlisted, false),
        (Visibility::Private, false),
        (Visibility::Unlisted, false),
        (Visibility::Limited, true),
        (Visibility::Unlisted, false),
        (Visibility::Public, true),
    ] {
        set_visibility(&ctx, &resolved, visibility).await;
        assert_eq!(listed(&ctx, &resolved.uid).await, expected, "{visibility:?}");
    }
}

#[tokio::test]
async fn restoring_a_deleted_unlisted_repo_keeps_it_out_of_the_visible_index() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let resolved = create(&ctx, "acme/hidden", Visibility::Unlisted).await;
    repo::delete(&ctx, &resolved, "acme/hidden").await.unwrap();
    repo::restore(&ctx, &resolved).await.unwrap();
    assert!(!listed(&ctx, &resolved.uid).await);
}

#[tokio::test]
async fn a_fork_of_an_unlisted_repo_is_unlisted_and_unindexed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let alice = gitquarry_test_support::account(&ctx, "alice", None).await;
    let source = create(&ctx, "acme/hidden", Visibility::Unlisted).await;
    let fork = repo::fork(
        &ctx,
        &source,
        "alice/hidden",
        None,
        OwnerRef::User(alice.uid),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap()
    .unwrap();
    let (config, _etag) = fork.remote.get_repo_config().await.unwrap();
    assert_eq!(config.visibility, Visibility::Unlisted);
    assert!(!listed(&ctx, &fork.uid).await);
}

/// Federation only ever sees `GET /_internal/repos/public[/name]`, so an
/// unlisted repo must be absent from both even when a stale index entry
/// still names it.
#[tokio::test]
async fn federation_never_sees_an_unlisted_repo() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let resolved = create(&ctx, "acme/hidden", Visibility::Unlisted).await;
    settings::set_visible_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, true).await;

    let resp = gitquarry_core::internal_api::handler_public_repos(State(Arc::clone(&ctx))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let repos: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(repos.is_empty(), "{repos:?}");

    let resp =
        gitquarry_core::internal_api::handler_public_repo(State(Arc::clone(&ctx)), Path("acme/hidden".to_string()))
            .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_config_written_directly_round_trips_unlisted() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = gitquarry_test_support::ctx(dir.path());
    let resolved = ctx.create_repo("acme/raw").await.unwrap().unwrap();
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/raw".to_string(),
        visibility: Visibility::Unlisted,
        ..Default::default()
    };
    settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();
    let (read, _etag) = settings::get_repo_config(ctx.backend().as_ref(), resolved.uid.as_str(), &ctx.cipher)
        .await
        .unwrap();
    assert_eq!(read, config);
}
