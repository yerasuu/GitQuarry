use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitquarry_federation::access;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::web::router;
use gitquarry_test_support::{Page, federation_state, send};
use gitquarry_wire::InstanceInfo;
use std::sync::Arc;

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn banned_peers_lists_only_banned_ones_with_reason_and_actor() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");
    let linked = Peer {
        name: "good-peer".to_string(),
        web_base_url: "https://good.example".to_string(),
        git_base_url: "https://good.example".to_string(),
        linked_at: String::new(),
        access_level: Default::default(),
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    let banned = Peer {
        name: "bad-peer".to_string(),
        web_base_url: "https://bad.example".to_string(),
        git_base_url: "https://bad.example".to_string(),
        linked_at: String::new(),
        access_level: Default::default(),
        banned: true,
        ban_reason: Some("spamming forks".to_string()),
        banned_at: Some("2026-08-06T00:00:00Z".to_string()),
        banned_by: "alice".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &linked, None)
        .await
        .unwrap();
    access::put_peer(state.backend.as_ref(), &state.cipher, &banned, None)
        .await
        .unwrap();

    let Page { status, body, .. } = send(&router(Arc::clone(&state)), get("/_api/banned-peers")).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let list = json.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["peer_name"], "bad-peer");
    assert_eq!(list[0]["ban_reason"], "spamming forks");
    assert_eq!(list[0]["banned_by"], "alice");
}

#[tokio::test]
async fn instance_get_reports_a_live_count_of_currently_banned_peers() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), "http://127.0.0.1:1");

    let body = send(&router(Arc::clone(&state)), get("/_api/instance")).await.body;
    let info: InstanceInfo = serde_json::from_str(&body).unwrap();
    assert_eq!(info.total_bans_issued, 0);

    for name in ["bad-1", "bad-2"] {
        let peer = Peer {
            name: name.to_string(),
            web_base_url: "https://bad.example".to_string(),
            git_base_url: "https://bad.example".to_string(),
            linked_at: String::new(),
            access_level: Default::default(),
            banned: true,
            ban_reason: Some("abuse".to_string()),
            banned_at: None,
            banned_by: "alice".to_string(),
            ..Default::default()
        };
        access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
            .await
            .unwrap();
    }

    let body = send(&router(Arc::clone(&state)), get("/_api/instance")).await.body;
    let info: InstanceInfo = serde_json::from_str(&body).unwrap();
    assert_eq!(info.total_bans_issued, 2);
}
