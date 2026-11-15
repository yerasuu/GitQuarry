//! `api::write::WriteRequest` as a write endpoint uses it: `dry_run` and
//! `Idempotency-Key`, read at the edge, decide whether the write happens.
//! No `/_api/v1` route writes yet, so these mount one that files numbered
//! "issues" and counts how many it filed.

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Extension, Router};
use gitquarry_core::api::auth::ApiPrincipal;
use gitquarry_core::api::error::{ApiError, ErrorCode};
use gitquarry_core::api::idempotency;
use gitquarry_core::api::write::{Mode, WriteRequest};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{ApiGrant, ApiScope, Permission};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::{Page, account, send};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::Notify;

#[derive(Default)]
struct Tracker {
    filed: AtomicU32,
    calls: AtomicU32,
    /// Calls that answer 500 before any is filed.
    fail_first: AtomicU32,
    /// When set, a call waits for it before filing.
    gate: Option<Arc<Notify>>,
}

async fn file_issue(
    State(ctx): State<Arc<AppContext>>,
    Extension(tracker): Extension<Arc<Tracker>>,
    principal: ApiPrincipal,
    write: WriteRequest,
) -> axum::response::Response {
    write
        .run(&ctx, &principal, |mode, body| async move {
            tracker.calls.fetch_add(1, Ordering::SeqCst);
            let body: Value = serde_json::from_slice(&body)
                .map_err(|_| ApiError::new(ErrorCode::InvalidParameter, "body must be JSON"))?;
            if mode == Mode::DryRun {
                return Ok(axum::Json(json!({ "would_file": body["title"] })).into_response());
            }
            if tracker
                .fail_first
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(ApiError::internal(&anyhow::anyhow!("storage down")));
            }
            if let Some(gate) = &tracker.gate {
                gate.notified().await;
            }
            let number = tracker.filed.fetch_add(1, Ordering::SeqCst) + 1;
            Ok((
                StatusCode::CREATED,
                [(header::LOCATION, format!("/issues/{number}"))],
                axum::Json(json!({ "number": number, "title": body["title"] })),
            )
                .into_response())
        })
        .await
}

fn app(ctx: &Arc<AppContext>, tracker: &Arc<Tracker>) -> Router {
    Router::new()
        .route("/issues", post(file_issue))
        .layer(Extension(Arc::clone(tracker)))
        .with_state(Arc::clone(ctx))
}

/// alice and her repo, which every token here is scoped to.
struct Env {
    ctx: Arc<AppContext>,
    owner: Uid,
    proj: Uid,
}

impl Env {
    async fn new(dir: &Path) -> Env {
        let ctx = Arc::new(gitquarry_test_support::ctx(dir));
        let owner = account(&ctx, "alice", None).await.uid;
        let proj = gitquarry_core::domain::repo::create(&ctx, "alice/proj", None, None, None, Visibility::Private)
            .await
            .unwrap()
            .unwrap()
            .uid;
        Env { ctx, owner, proj }
    }

    /// A new API token of alice's, returning its secret.
    async fn token(&self) -> String {
        let grant = ApiGrant {
            scopes: ApiScope::ALL.into_iter().collect(),
            paths: BTreeMap::new(),
        };
        let scopes = BTreeMap::from([(self.proj.clone(), Permission::Developer)]);
        domain::pat::create_api(&self.ctx, &self.owner, "agent", scopes, grant, None)
            .await
            .unwrap()
            .unwrap()
            .1
    }
}

async fn file(app: &Router, secret: &str, uri: &str, key: Option<&str>, body: &str) -> Page {
    let mut request = Request::post(uri).header(header::AUTHORIZATION, format!("Bearer {secret}"));
    if let Some(key) = key {
        request = request.header(idempotency::HEADER, key);
    }
    send(app, request.body(Body::from(body.to_string())).unwrap()).await
}

fn json_body(page: &Page) -> Value {
    serde_json::from_str(&page.body).unwrap_or_else(|_| panic!("not JSON: {}", page.body))
}

#[tokio::test]
async fn a_dry_run_describes_the_write_and_applies_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;

    let page = file(&app, &secret, "/issues?dry_run=true", None, r#"{"title":"x"}"#).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(json_body(&page)["would_file"], "x");
    assert_eq!(tracker.filed.load(Ordering::SeqCst), 0);

    for uri in ["/issues", "/issues?dry_run=false"] {
        let page = file(&app, &secret, uri, None, "{}").await;
        assert_eq!(page.status, StatusCode::CREATED, "{uri}: {}", page.body);
    }
    assert_eq!(tracker.filed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_misspelt_dry_run_is_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;

    for uri in ["/issues?dry_run=ture", "/issues?dry_run=1", "/issues?dry_run="] {
        let page = file(&app, &secret, uri, None, "{}").await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(json_body(&page)["code"], "invalid_parameter", "{uri}");
    }
    assert_eq!(tracker.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_body_over_the_router_limit_is_request_too_large() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker).layer(axum::extract::DefaultBodyLimit::max(16));
    let secret = env.token().await;

    let page = file(&app, &secret, "/issues", None, &"x".repeat(64)).await;
    assert_eq!(page.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json_body(&page)["code"], "request_too_large");
    assert_eq!(tracker.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_repeated_key_and_body_replays_the_first_answer_without_filing_again() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;
    let body = r#"{"title":"crash on push"}"#;

    let first = file(&app, &secret, "/issues", Some(r#""k-1""#), body).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.body);
    assert!(first.headers.get(idempotency::REPLAYED_HEADER).is_none());

    // The bare spelling of the same sf-string is the same key.
    for key in [r#""k-1""#, "k-1"] {
        let again = file(&app, &secret, "/issues", Some(key), body).await;
        assert_eq!(again.status, StatusCode::CREATED, "{}", again.body);
        assert_eq!(again.body, first.body);
        assert_eq!(again.headers[header::LOCATION], "/issues/1");
        assert_eq!(again.headers[header::CONTENT_TYPE], "application/json");
        assert_eq!(again.headers[idempotency::REPLAYED_HEADER], "true");
    }
    assert_eq!(tracker.filed.load(Ordering::SeqCst), 1);
    assert_eq!(tracker.calls.load(Ordering::SeqCst), 1);

    let other = file(&app, &secret, "/issues", Some("k-2"), body).await;
    assert_eq!(json_body(&other)["number"], 2);
}

#[tokio::test]
async fn the_same_key_with_another_request_is_a_422() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;

    let first = file(&app, &secret, "/issues", Some("k"), r#"{"title":"a"}"#).await;
    assert_eq!(first.status, StatusCode::CREATED);

    for (uri, body) in [("/issues", r#"{"title":"b"}"#), ("/issues?x=1", r#"{"title":"a"}"#)] {
        let page = file(&app, &secret, uri, Some("k"), body).await;
        assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
        let error = json_body(&page);
        assert_eq!(error["code"], "idempotency_key_reused");
        assert!(error["message"].is_string() && error["hint"].is_string(), "{error}");
    }
    assert_eq!(tracker.filed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn keys_belong_to_one_token() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let one = env.token().await;
    let two = env.token().await;

    let a = file(&app, &one, "/issues", Some("shared"), "{}").await;
    let b = file(&app, &two, "/issues", Some("shared"), "{}").await;
    assert_eq!(json_body(&a)["number"], 1);
    assert_eq!(json_body(&b)["number"], 2);
    assert!(b.headers.get(idempotency::REPLAYED_HEADER).is_none());
}

#[tokio::test]
async fn a_server_error_releases_the_key_for_a_retry() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker {
        fail_first: AtomicU32::new(1),
        ..Tracker::default()
    });
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;

    let failed = file(&app, &secret, "/issues", Some("k"), "{}").await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    let retried = file(&app, &secret, "/issues", Some("k"), "{}").await;
    assert_eq!(retried.status, StatusCode::CREATED, "{}", retried.body);
    assert!(retried.headers.get(idempotency::REPLAYED_HEADER).is_none());
    assert_eq!(tracker.filed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_client_error_is_kept_and_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;

    for _ in 0..2 {
        let page = file(&app, &secret, "/issues", Some("k"), "not json").await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST);
        assert_eq!(json_body(&page)["code"], "invalid_parameter");
    }
    assert_eq!(tracker.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_repeat_while_the_first_still_runs_is_a_409() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let gate = Arc::new(Notify::new());
    let tracker = Arc::new(Tracker {
        gate: Some(Arc::clone(&gate)),
        ..Tracker::default()
    });
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;

    let first = tokio::spawn({
        let app = app.clone();
        let secret = secret.clone();
        async move { file(&app, &secret, "/issues", Some("k"), "{}").await }
    });
    while tracker.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let racing = file(&app, &secret, "/issues", Some("k"), "{}").await;
    assert_eq!(racing.status, StatusCode::CONFLICT, "{}", racing.body);
    assert_eq!(json_body(&racing)["code"], "idempotency_key_in_use");

    gate.notify_one();
    assert_eq!(first.await.unwrap().status, StatusCode::CREATED);
    let replayed = file(&app, &secret, "/issues", Some("k"), "{}").await;
    assert_eq!(replayed.headers[idempotency::REPLAYED_HEADER], "true");
    assert_eq!(tracker.filed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_dry_run_neither_takes_nor_replays_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;
    let body = r#"{"title":"t"}"#;

    let dry = file(&app, &secret, "/issues?dry_run=true", Some("k"), body).await;
    assert_eq!(dry.status, StatusCode::OK);
    let real = file(&app, &secret, "/issues", Some("k"), body).await;
    assert_eq!(real.status, StatusCode::CREATED, "{}", real.body);
    assert!(real.headers.get(idempotency::REPLAYED_HEADER).is_none());
    let dry = file(&app, &secret, "/issues?dry_run=true", Some("k"), body).await;
    assert_eq!(json_body(&dry)["would_file"], "t");
    assert_eq!(tracker.filed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_malformed_key_is_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;
    let long = "k".repeat(256);

    for key in ["", r#""""#, r#""open"#, r#""a"b""#, "a\"b", long.as_str()] {
        let page = file(&app, &secret, "/issues", Some(key), "{}").await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{key:?}");
        assert_eq!(json_body(&page)["code"], "invalid_parameter", "{key:?}");
    }
    assert_eq!(tracker.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stored_answers_are_sealed_and_a_sweep_keeps_live_keys() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new(dir.path()).await;
    let tracker = Arc::new(Tracker::default());
    let app = app(&env.ctx, &tracker);
    let secret = env.token().await;
    let marker = "plaintext-marker-7f3a";

    let first = file(
        &app,
        &secret,
        "/issues",
        Some("k"),
        &format!(r#"{{"title":"{marker}"}}"#),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);

    let backend = env.ctx.backend();
    let records = backend.list_prefix(".conf/api-idempotency/").await.unwrap();
    assert_eq!(records.len(), 1, "{records:?}");
    let (sealed, _) = backend.get(&records[0].0).await.unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains(marker));

    assert_eq!(idempotency::sweep_expired(&env.ctx).await.unwrap(), 0);
    let again = file(
        &app,
        &secret,
        "/issues",
        Some("k"),
        &format!(r#"{{"title":"{marker}"}}"#),
    )
    .await;
    assert_eq!(again.headers[idempotency::REPLAYED_HEADER], "true");
}
