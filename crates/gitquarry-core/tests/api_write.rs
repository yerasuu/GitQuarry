//! `api::write::WriteRequest` as a write endpoint uses it: `dry_run`, read
//! at the edge, decides whether the write happens. No `/_api/v1` route
//! writes yet, so these mount one that counts what it applies.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use gitquarry_core::api::write::{Mode, WriteRequest};
use gitquarry_test_support::{Page, send};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// A write that counts each time it applies and echoes the body it was given.
fn app(applied: &Arc<AtomicU32>) -> Router {
    let applied = Arc::clone(applied);
    Router::new().route(
        "/write",
        post(move |write: WriteRequest| async move {
            write
                .run(|mode, body| async move {
                    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    if mode == Mode::Apply {
                        applied.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(axum::Json(json!({ "dry_run": mode == Mode::DryRun, "body": body })).into_response())
                })
                .await
        }),
    )
}

async fn post_to(app: &Router, uri: &str, body: &str) -> Page {
    send(app, Request::post(uri).body(Body::from(body.to_string())).unwrap()).await
}

#[tokio::test]
async fn a_dry_run_describes_the_write_and_applies_nothing() {
    let applied = Arc::new(AtomicU32::new(0));
    let app = app(&applied);

    let page = post_to(&app, "/write?dry_run=true", r#"{"title":"x"}"#).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let body: Value = serde_json::from_str(&page.body).unwrap();
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["body"]["title"], "x");
    assert_eq!(applied.load(Ordering::Relaxed), 0);

    for uri in ["/write", "/write?dry_run=false"] {
        let page = post_to(&app, uri, "{}").await;
        assert_eq!(page.status, StatusCode::OK, "{uri}: {}", page.body);
    }
    assert_eq!(applied.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn a_misspelt_dry_run_is_refused_before_anything_runs() {
    let applied = Arc::new(AtomicU32::new(0));
    let app = app(&applied);

    for uri in ["/write?dry_run=ture", "/write?dry_run=1", "/write?dry_run="] {
        let page = post_to(&app, uri, "{}").await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{uri}");
        let body: Value = serde_json::from_str(&page.body).unwrap();
        assert_eq!(body["code"], "invalid_parameter", "{uri}");
    }
    assert_eq!(applied.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn a_body_over_the_router_limit_is_request_too_large() {
    let applied = Arc::new(AtomicU32::new(0));
    let app = app(&applied).layer(axum::extract::DefaultBodyLimit::max(16));

    let page = post_to(&app, "/write", &"x".repeat(64)).await;
    assert_eq!(page.status, StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = serde_json::from_str(&page.body).unwrap();
    assert_eq!(body["code"], "request_too_large");
    assert_eq!(applied.load(Ordering::Relaxed), 0);
}
