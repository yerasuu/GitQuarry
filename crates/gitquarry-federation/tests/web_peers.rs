//! `/_peers` as core's proxy reaches it: through `web::router`, with the
//! session identity and CSRF token core forwards as headers.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_federation::access;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use gitquarry_test_support::{federation_state, send};
use std::sync::Arc;

async fn get_page(state: &Arc<FederationState>, user: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::get("/_peers").header("x-gitquarry-csrf", "csrf-from-core");
    if let Some(user) = user {
        req = req.header("x-gitquarry-user", user);
    }
    let page = send(&router(Arc::clone(state)), req.body(Body::empty()).unwrap()).await;
    (page.status, page.body)
}

async fn put_peer(state: &FederationState, name: &str) {
    let peer = Peer {
        name: name.to_string(),
        web_base_url: "https://acme.example:8080".to_string(),
        git_base_url: "ssh://acme.example:2222".to_string(),
        linked_at: "2026-08-05T00:00:00Z".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn is_refused_without_the_user_core_forwards() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    assert_eq!(get_page(&state, None).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lists_each_peer_with_a_delete_button_carrying_cores_csrf_token() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    put_peer(&state, "acme-instance").await;

    let (status, html) = get_page(&state, Some("alice")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("acme-instance"));
    assert!(html.contains(r#"value="delete_peer""#));
    assert!(html.contains(r#"name="csrf" value="csrf-from-core""#));
}

#[tokio::test]
async fn shows_the_empty_hint_without_peers() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let (_, html) = get_page(&state, Some("alice")).await;
    assert!(html.contains("no linked instances yet"));
}

#[tokio::test]
async fn escapes_a_hostile_peer_name() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    put_peer(&state, r#"<script>alert('x')&"y"</script>"#).await;

    let (_, html) = get_page(&state, Some("alice")).await;
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&amp;&quot;y&quot;&lt;/script&gt;"));
}
