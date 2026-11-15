//! `/_api/v1` as a client sees it: bearer authentication, the structured
//! error body, and cursor pagination, through `api::router` and `whoami`.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::api;
use gitquarry_core::api::error::ErrorCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{ApiGrant, ApiScope, Permission};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::{Page, account, send};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

fn app(ctx: &Arc<AppContext>) -> axum::Router {
    axum::Router::new().merge(api::router(ctx)).with_state(Arc::clone(ctx))
}

async fn repo(ctx: &AppContext, full_name: &str) -> Uid {
    gitquarry_core::domain::repo::create(ctx, full_name, None, None, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap()
        .uid
}

/// Every API scope, so these tests see only the per-repo ceiling.
fn every_scope() -> ApiGrant {
    ApiGrant {
        scopes: ApiScope::ALL.into_iter().collect(),
        paths: BTreeMap::new(),
    }
}

/// alice's API token over `scopes`, returning its secret.
async fn token(ctx: &AppContext, owner: &Uid, scopes: BTreeMap<Uid, Permission>) -> String {
    domain::pat::create_api(ctx, owner, "agent", scopes, every_scope(), None)
        .await
        .unwrap()
        .unwrap()
        .1
}

async fn get(ctx: &Arc<AppContext>, path: &str, authorization: Option<&str>) -> Page {
    let mut request = Request::get(path);
    if let Some(value) = authorization {
        request = request.header(header::AUTHORIZATION, value);
    }
    send(&app(ctx), request.body(Body::empty()).unwrap()).await
}

fn json(page: &Page) -> Value {
    assert_eq!(page.headers[header::CONTENT_TYPE], "application/json", "{}", page.body);
    serde_json::from_str(&page.body).unwrap()
}

/// The error body's `code`, after checking the body has all three keys.
fn error_code(page: &Page) -> String {
    let body = json(page);
    assert!(body["message"].is_string(), "{body}");
    assert!(body.get("hint").is_some(), "{body}");
    body["code"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn whoami_names_the_user_the_token_and_its_capped_permission() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice", None).await;
    let proj = repo(&ctx, "alice/proj").await;
    // alice owns the repo, but this token only reads it.
    let secret = token(&ctx, &alice.uid, BTreeMap::from([(proj.clone(), Permission::Reporter)])).await;

    let page = get(&ctx, "/_api/v1/whoami", Some(&format!("Bearer {secret}"))).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let body = json(&page);
    assert_eq!(body["user"]["username"], "alice");
    assert_eq!(body["user"]["uid"], alice.uid.as_str());
    assert_eq!(body["token"]["label"], "agent");
    assert_eq!(body["scopes"]["items"][0]["repo_uid"], proj.as_str());
    assert_eq!(body["scopes"]["items"][0]["repo"], "alice/proj");
    assert_eq!(body["scopes"]["items"][0]["ceiling"], "reporter");
    assert_eq!(body["scopes"]["items"][0]["permission"], "reporter");
    assert_eq!(body["scopes"]["items"][0]["tracker_permission"], "reporter");
    assert_eq!(body["scopes"]["items"][0]["paths"], Value::Null);
    assert_eq!(body["token"]["scopes"].as_array().unwrap().len(), ApiScope::ALL.len());
    assert_eq!(body["scopes"]["next_cursor"], Value::Null);
    assert!(!page.body.contains(&secret), "the secret must never be echoed");
}

#[tokio::test]
async fn the_bearer_scheme_is_matched_without_regard_to_case() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice", None).await;
    let proj = repo(&ctx, "alice/proj").await;
    let secret = token(&ctx, &alice.uid, BTreeMap::from([(proj, Permission::Reporter)])).await;

    let page = get(&ctx, "/_api/v1/whoami", Some(&format!("bearer {secret}"))).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
}

#[tokio::test]
async fn a_missing_token_is_a_401_asking_for_a_bearer() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));

    for authorization in [None, Some("Basic YWxpY2U6cHc=")] {
        let page = get(&ctx, "/_api/v1/whoami", authorization).await;
        assert_eq!(page.status, StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&page), "unauthenticated");
        assert_eq!(page.headers[header::WWW_AUTHENTICATE], r#"Bearer realm="gitquarry""#);
    }
}

#[tokio::test]
async fn an_unknown_or_malformed_token_is_a_401_invalid_token() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let unknown = format!("Bearer gqpat_{}", "0".repeat(40));

    for authorization in [unknown.as_str(), "Bearer not-a-token", "Bearer gqpat_ZZ", "Bearer "] {
        let page = get(&ctx, "/_api/v1/whoami", Some(authorization)).await;
        assert_eq!(page.status, StatusCode::UNAUTHORIZED, "{authorization}");
        assert_eq!(error_code(&page), "invalid_token", "{authorization}");
        assert!(
            page.headers[header::WWW_AUTHENTICATE]
                .to_str()
                .unwrap()
                .contains(r#"error="invalid_token""#)
        );
    }
}

#[tokio::test]
async fn a_revoked_token_stops_working() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice", None).await;
    let proj = repo(&ctx, "alice/proj").await;
    let (record, secret) = domain::pat::create_api(
        &ctx,
        &alice.uid,
        "agent",
        BTreeMap::from([(proj, Permission::Reporter)]),
        every_scope(),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    domain::pat::revoke(&ctx, &alice.uid, record.uid.as_str(), false)
        .await
        .unwrap();

    let page = get(&ctx, "/_api/v1/whoami", Some(&format!("Bearer {secret}"))).await;
    assert_eq!(page.status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&page), "invalid_token");
}

#[tokio::test]
async fn scopes_page_through_with_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice", None).await;
    let mut scopes = BTreeMap::new();
    for name in ["alice/a", "alice/b", "alice/c"] {
        scopes.insert(repo(&ctx, name).await, Permission::Developer);
    }
    let secret = token(&ctx, &alice.uid, scopes.clone()).await;
    let bearer = format!("Bearer {secret}");

    let first = json(&get(&ctx, "/_api/v1/whoami?limit=2", Some(&bearer)).await);
    let cursor = first["scopes"]["next_cursor"].as_str().unwrap().to_string();
    let second = json(&get(&ctx, &format!("/_api/v1/whoami?limit=2&cursor={cursor}"), Some(&bearer)).await);
    assert_eq!(second["scopes"]["next_cursor"], Value::Null);

    let seen: Vec<String> = [&first, &second]
        .iter()
        .flat_map(|page| page["scopes"]["items"].as_array().unwrap().iter())
        .map(|item| item["repo_uid"].as_str().unwrap().to_string())
        .collect();
    let expected: Vec<String> = scopes.keys().map(|uid| uid.as_str().to_string()).collect();
    assert_eq!(seen, expected);
}

#[tokio::test]
async fn a_malformed_cursor_is_a_400_invalid_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice", None).await;
    let proj = repo(&ctx, "alice/proj").await;
    let secret = token(&ctx, &alice.uid, BTreeMap::from([(proj, Permission::Reporter)])).await;
    let bearer = format!("Bearer {secret}");

    for cursor in ["garbage!", "AAAA", &"A".repeat(4096)] {
        let page = get(&ctx, &format!("/_api/v1/whoami?cursor={cursor}"), Some(&bearer)).await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&page), "invalid_cursor");
    }
}

#[tokio::test]
async fn one_tokens_cursor_does_not_page_another_tokens_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice", None).await;
    let mut scopes = BTreeMap::new();
    for name in ["alice/a", "alice/b"] {
        scopes.insert(repo(&ctx, name).await, Permission::Reporter);
    }
    let first = token(&ctx, &alice.uid, scopes.clone()).await;
    let second = token(&ctx, &alice.uid, scopes).await;

    let page = json(&get(&ctx, "/_api/v1/whoami?limit=1", Some(&format!("Bearer {first}"))).await);
    let cursor = page["scopes"]["next_cursor"].as_str().unwrap();
    let replayed = get(
        &ctx,
        &format!("/_api/v1/whoami?limit=1&cursor={cursor}"),
        Some(&format!("Bearer {second}")),
    )
    .await;
    assert_eq!(replayed.status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&replayed), "invalid_cursor");
}

#[test]
fn a_cursor_opens_only_under_the_scope_it_was_sealed_for() {
    let cipher = gitquarry_test_support::cipher();
    let cursor = api::cursor::seal(&cipher, "repo-a", "position").unwrap();

    assert_eq!(api::cursor::open(&cipher, "repo-a", &cursor).unwrap(), "position");
    let err = api::cursor::open(&cipher, "repo-b", &cursor).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidCursor);
}

#[tokio::test]
async fn an_out_of_range_limit_is_a_400_invalid_parameter() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice", None).await;
    let proj = repo(&ctx, "alice/proj").await;
    let secret = token(&ctx, &alice.uid, BTreeMap::from([(proj, Permission::Reporter)])).await;

    for limit in ["0", "101", "-1", "many"] {
        let page = get(
            &ctx,
            &format!("/_api/v1/whoami?limit={limit}"),
            Some(&format!("Bearer {secret}")),
        )
        .await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{limit}");
        assert_eq!(error_code(&page), "invalid_parameter");
    }
}

#[tokio::test]
async fn unknown_paths_and_methods_answer_json() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));

    let page = get(&ctx, "/_api/v1/nope", None).await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&page), "not_found");

    let post = Request::post("/_api/v1/whoami").body(Body::empty()).unwrap();
    let page = send(&app(&ctx), post).await;
    assert_eq!(page.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(error_code(&page), "method_not_allowed");
}
