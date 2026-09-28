use gitquarry_core::internal_api::*;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use gitquarry_core::app::AppContext;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_i18n::Locale;
use s3db::LocalBackend;
use s3db_crypto::SALT_LEN;
use std::collections::HashMap;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> Arc<AppContext> {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[31u8; SALT_LEN]).unwrap());
    Arc::new(AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    })
}

async fn body_status(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn authorize_reflects_a_real_owner_grant() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap();
    ctx.create_repo("alice/proj").await.unwrap();

    let body = serde_json::to_vec(&serde_json::json!({
        "username": "alice", "repo": "alice/proj", "require": "developer"
    }))
    .unwrap();
    let resp = handler_authorize(State(Arc::clone(&ctx)), Bytes::from(body)).await;
    let (status, json) = body_status(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ok"], true);
}

#[tokio::test]
async fn authorize_rejects_a_stranger() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    ctx.create_repo("alice/proj").await.unwrap();

    let body = serde_json::to_vec(&serde_json::json!({
        "username": "mallory", "repo": "alice/proj", "require": "reporter"
    }))
    .unwrap();
    let resp = handler_authorize(State(Arc::clone(&ctx)), Bytes::from(body)).await;
    let (_status, json) = body_status(resp).await;
    assert_eq!(json["ok"], false);
}

#[tokio::test]
async fn authorize_and_summarize_returns_a_private_repos_summary_only_to_someone_entitled() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "alice",
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap();
    let resolved = ctx.create_repo("alice/proj").await.unwrap().unwrap();
    let (mut config, etag) = resolved.remote.get_repo_config().await.unwrap();
    config.full_name = "alice/proj".to_string();
    config.visibility = Visibility::Private;
    storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let body = serde_json::to_vec(&serde_json::json!({
        "username": "alice", "uid": resolved.uid.as_str(), "require": "reporter"
    }))
    .unwrap();
    let resp = handler_authorize_and_summarize(State(Arc::clone(&ctx)), Bytes::from(body)).await;
    let (_status, json) = body_status(resp).await;
    assert_eq!(json["ok"], true);
    assert_eq!(json["repo"]["name"], "alice/proj");

    let body = serde_json::to_vec(&serde_json::json!({
        "username": "mallory", "uid": resolved.uid.as_str(), "require": "reporter"
    }))
    .unwrap();
    let resp = handler_authorize_and_summarize(State(Arc::clone(&ctx)), Bytes::from(body)).await;
    let (_status, json) = body_status(resp).await;
    assert_eq!(json["ok"], false);
    assert!(json.get("repo").is_none());
}

#[tokio::test]
async fn uid_for_name_finds_a_private_repo_by_exact_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = ctx.create_repo("alice/secret").await.unwrap().unwrap();
    let (mut config, etag) = resolved.remote.get_repo_config().await.unwrap();
    config.full_name = "alice/secret".to_string();
    config.visibility = Visibility::Private;
    storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let resp = handler_uid_for_name(
        State(Arc::clone(&ctx)),
        Query(HashMap::from([("name".to_string(), "alice/secret".to_string())])),
    )
    .await;
    let (status, json) = body_status(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["uid"], resolved.uid.as_str());
}

#[tokio::test]
async fn uid_for_name_404s_for_a_nonexistent_repo() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resp = handler_uid_for_name(
        State(ctx),
        Query(HashMap::from([("name".to_string(), "nobody/nothing".to_string())])),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn public_repos_lists_only_visible_public_repos() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let resolved = ctx.create_repo("acme/demo").await.unwrap().unwrap();
    let config = storage::entities::RepoConfig {
        uid: resolved.uid.clone(),
        full_name: "acme/demo".to_string(),
        visibility: Visibility::Public,
        ..Default::default()
    };
    storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();
    storage::settings::set_visible_index(ctx.backend().as_ref(), &ctx.cipher, &resolved.uid, true).await;

    let resp = handler_public_repos(State(Arc::clone(&ctx))).await;
    let (status, json) = body_status(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["name"], "acme/demo");
}

#[tokio::test]
async fn fork_from_remote_rejects_a_namespace_the_acting_user_doesnt_control() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    gitquarry_core::auth::domain::user::create(
        &ctx,
        gitquarry_core::auth::domain::user::NewUser {
            username: "mallory",
            email: None,
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap();

    let body = serde_json::to_vec(&serde_json::json!({
        "source_git_base_url": "http://127.0.0.1:1",
        "source_repo_name": "acme/demo",
        "dest_full_name": "alice/demo",
        "dest_owner_kind": "user",
        "dest_owner_uid": "not-mallorys-uid",
        "description": null,
        "peer_label": "instance-b",
        "acting_username": "mallory",
    }))
    .unwrap();
    let resp = handler_fork_from_remote(State(ctx), Bytes::from(body)).await;
    let (status, json) = body_status(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ok"], false);
    assert!(json["reason"].as_str().unwrap().contains("doesn't control"));
}
