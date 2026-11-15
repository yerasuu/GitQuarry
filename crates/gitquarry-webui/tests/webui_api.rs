//! `/_api/v1` mounted on the web UI's router: the API's own JSON answers
//! must win over the UI's catch-all page and its method-not-allowed redirect,
//! and its limits must hold there as on the API's own router.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::api::limits::ApiLimits;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{ApiGrant, ApiScope, Permission};
use gitquarry_core::config::ApiSection;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{account, ctx, send};
use std::collections::BTreeMap;
use std::sync::Arc;

#[tokio::test]
async fn the_web_ui_router_serves_the_api_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(ctx(dir.path())));

    let page = send(&app, Request::get("/_api/v1/whoami").body(Body::empty()).unwrap()).await;
    assert_eq!(page.status, StatusCode::UNAUTHORIZED);
    assert_eq!(page.headers[header::CONTENT_TYPE], "application/json");
    assert!(page.body.contains(r#""code":"unauthenticated""#), "{}", page.body);

    let page = send(&app, Request::post("/_api/v1/whoami").body(Body::empty()).unwrap()).await;
    assert_eq!(page.status, StatusCode::METHOD_NOT_ALLOWED);

    let page = send(
        &app,
        Request::get("/_api/v1/no/such/thing").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert_eq!(page.headers[header::CONTENT_TYPE], "application/json");
}

#[tokio::test]
async fn the_web_ui_router_applies_the_api_rate_limit() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(AppContext {
        api_limits: ApiLimits::new(&ApiSection {
            requests_per_minute: 1,
            ..ApiSection::default()
        }),
        ..ctx(dir.path())
    });
    let alice = account(&ctx, "alice", None).await;
    let proj = gitquarry_core::domain::repo::create(&ctx, "alice/proj", None, None, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap()
        .uid;
    let grant = ApiGrant {
        scopes: ApiScope::ALL.into_iter().collect(),
        paths: BTreeMap::new(),
    };
    let (_, secret) = domain::pat::create_api(
        &ctx,
        &alice.uid,
        "agent",
        BTreeMap::from([(proj, Permission::Reporter)]),
        grant,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let request = || {
        Request::get("/_api/v1/whoami")
            .header(header::AUTHORIZATION, format!("Bearer {secret}"))
            .body(Body::empty())
            .unwrap()
    };

    let page = send(&app, request()).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.headers["ratelimit-remaining"], "0");
    let page = send(&app, request()).await;
    assert_eq!(page.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(page.headers.get(header::RETRY_AFTER).is_some());
}
