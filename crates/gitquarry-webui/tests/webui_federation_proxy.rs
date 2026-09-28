use axum::body::Bytes;
use axum::http::header::COOKIE;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use gitquarry_webui::webui::federation_proxy::*;

/// What federation embeds in its own rendered forms: the token core's
/// `csrf_field` would render, not the session cookie behind it.
#[test]
fn session_csrf_value_extracts_the_rendered_token() {
    let mut headers = HeaderMap::new();
    headers.insert(COOKIE, "web-session=sealed-abc".parse().unwrap());
    let value = session_csrf_value(&headers);
    assert!(!value.is_empty());
    assert_ne!(
        value, "sealed-abc",
        "session cookie forwarded to federation as the csrf token"
    );
}

#[test]
fn session_csrf_value_is_empty_without_a_session() {
    assert_eq!(session_csrf_value(&HeaderMap::new()), "");
}

#[test]
fn is_hop_by_hop_flags_connection_framing_but_not_ordinary_headers() {
    assert!(is_hop_by_hop(&header::CONNECTION));
    assert!(is_hop_by_hop(&header::HOST));
    assert!(is_hop_by_hop(&header::CONTENT_LENGTH));
    assert!(!is_hop_by_hop(&header::CONTENT_TYPE));
    assert!(!is_hop_by_hop(&header::AUTHORIZATION));
}

/// Spins up a tiny stand-in for `gitquarry-federation`'s own listener —
/// echoes back whether `X-Internal-Token` matched and the request body
/// it received — then drives `send` (what every proxy handler in this
/// file ultimately calls) against it over a real socket.
async fn spawn_fake_federation(expected_token: &'static str) -> String {
    async fn handler(headers: HeaderMap, body: Bytes) -> Response {
        let ok = headers
            .get("x-internal-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|got| got == "expected-token");
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::to_vec(&serde_json::json!({ "token_ok": ok, "echo": String::from_utf8_lossy(&body) })).unwrap(),
        )
            .into_response()
    }
    let _ = expected_token;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/{*path}", any(handler));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn send_forwards_the_internal_token_and_body_and_relays_the_response() {
    let base_url = spawn_fake_federation("expected-token").await;
    let federation = gitquarry_core::app::FederationLink {
        base_url,
        internal_token: "expected-token".to_string(),
        http: reqwest::Client::new(),
    };
    let resp = send(
        &federation,
        reqwest::Method::POST,
        "/_api/pulls",
        &[],
        Bytes::from_static(b"hello"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["token_ok"], true);
    assert_eq!(json["echo"], "hello");
}

#[tokio::test]
async fn send_reports_a_bad_gateway_when_federation_is_unreachable() {
    let federation = gitquarry_core::app::FederationLink {
        base_url: "http://127.0.0.1:1".to_string(),
        internal_token: "tok".to_string(),
        http: reqwest::Client::new(),
    };
    let resp = send(&federation, reqwest::Method::GET, "/_api/instance", &[], Bytes::new()).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

#[test]
fn passthrough_headers_carries_only_authorization() {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, "Bearer token_abc".parse().unwrap());
    headers.insert(COOKIE, "web-session=sealed".parse().unwrap());
    let forwarded = passthrough_headers(&headers);
    assert_eq!(forwarded, vec![("authorization", "Bearer token_abc".to_string())]);
}

#[test]
fn passthrough_headers_empty_without_a_bearer_token() {
    assert!(passthrough_headers(&HeaderMap::new()).is_empty());
}

/// Regression test: a proxied `x-www-form-urlencoded` POST used to
/// arrive at federation with no `Content-Type` at all, since neither
/// `passthrough_headers` nor the session-gated proxies' own `extra`
/// vecs forwarded it — axum's `Form` extractor there 415s without it.
/// Caught via manual end-to-end testing (unit tests alone never drove
/// a real cross-process request), fixed by `content_type_header`.
#[test]
fn passthrough_headers_forwards_content_type_when_present() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/x-www-form-urlencoded".parse().unwrap(),
    );
    let forwarded = passthrough_headers(&headers);
    assert_eq!(
        forwarded,
        vec![("content-type", "application/x-www-form-urlencoded".to_string())]
    );
}

#[test]
fn content_type_header_empty_without_one() {
    assert!(content_type_header(&HeaderMap::new()).is_empty());
}

#[test]
fn traversal_free_accepts_ordinary_api_paths() {
    assert!(traversal_free("/_api/instance"));
    assert!(traversal_free("/_api/repos/team/project"));
    assert!(traversal_free("/_api/repos/team/project?ref=main"));
    // A repo name may legitimately contain dots, just never a bare `..` segment.
    assert!(traversal_free("/_api/repos/team/my.repo.name"));
}

#[test]
fn traversal_free_rejects_dot_segments_that_would_climb_out_of_the_api_surface() {
    assert!(!traversal_free("/_api/repos/../../_internal-out/peers"));
    assert!(!traversal_free("/_api/repos/%2e%2e/%2e%2e/_peers"));
    assert!(!traversal_free("/_api/repos/%2E%2E/_peers"));
    assert!(!traversal_free("/_api/repos/./_peers"));
    // The query half is federation's own business — only the path can traverse.
    assert!(traversal_free("/_api/repos/team/project?path=../etc"));
}

#[test]
fn proxied_csrf_reads_the_field_out_of_a_form_body() {
    let body = Bytes::from_static(b"action=ban_peer&csrf=sealed-abc&name=evil");
    assert_eq!(proxied_csrf(&body), "sealed-abc");
}

#[test]
fn proxied_csrf_is_empty_when_the_form_carries_no_token() {
    assert_eq!(proxied_csrf(&Bytes::from_static(b"action=ban_peer")), "");
    assert_eq!(proxied_csrf(&Bytes::new()), "");
}

#[test]
fn proxied_csrf_percent_decodes_the_value() {
    let body = Bytes::from_static(b"csrf=a%2Bb%3Dc");
    assert_eq!(proxied_csrf(&body), "a+b=c");
}
