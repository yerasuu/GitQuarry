//! `/_peers` as core's proxy reaches it: through `web::router`, with the
//! session identity and CSRF token core forwards as headers.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_federation::access;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::gossip::GossipTable;
use gitquarry_federation::rate_limit::PeerRateLimiter;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use s3db::{BackendConfig, LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

fn test_state(root: &std::path::Path) -> Arc<FederationState> {
    let bucket = root.to_string_lossy().to_string();
    let backend = LocalBackend::new(&LocalParams {
        bucket: bucket.clone(),
        engine: LocalEngine::Redb,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[21u8; s3db_crypto::SALT_LEN]).unwrap();
    Arc::new(FederationState {
        backend: Arc::new(backend),
        cipher: Arc::new(cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
        backend_config: BackendConfig::Local(LocalParams {
            bucket,
            engine: LocalEngine::Redb,
        }),
        gossip: Arc::new(GossipTable::default()),
        self_announce: None,
        rate_limiter: Arc::new(PeerRateLimiter::default()),
        rate_limit_public_max: 60,
        rate_limit_public_window: Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: Duration::from_secs(60),
        rate_limit_peer_override_by_ip: true,
    })
}

async fn get_page(state: &Arc<FederationState>, user: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::get("/_peers").header("x-gitquarry-csrf", "csrf-from-core");
    if let Some(user) = user {
        req = req.header("x-gitquarry-user", user);
    }
    let resp = router(Arc::clone(state))
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
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
    let state = test_state(dir.path());
    assert_eq!(get_page(&state, None).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lists_each_peer_with_a_delete_button_carrying_cores_csrf_token() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
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
    let state = test_state(dir.path());
    let (_, html) = get_page(&state, Some("alice")).await;
    assert!(html.contains("no linked instances yet"));
}

#[tokio::test]
async fn escapes_a_hostile_peer_name() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    put_peer(&state, r#"<script>alert('x')&"y"</script>"#).await;

    let (_, html) = get_page(&state, Some("alice")).await;
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&amp;&quot;y&quot;&lt;/script&gt;"));
}
