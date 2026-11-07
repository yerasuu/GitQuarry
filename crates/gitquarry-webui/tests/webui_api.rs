//! `/_api/v1` mounted on the web UI's router: the API's own JSON answers
//! must win over the UI's catch-all page and its method-not-allowed redirect.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_test_support::{ctx, send};
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
