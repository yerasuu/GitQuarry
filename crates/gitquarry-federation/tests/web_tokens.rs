//! `/_account/tokens` through `web::router`, with a stand-in for the one
//! core endpoint token creation needs (`/repos/uid-for-name`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web::router;
use gitquarry_federation::{access, domain};
use gitquarry_test_support::{federation_state, send};
use gitquarry_wire::Uid;
use std::sync::Arc;

const REPO_UID: &str = "repo-uid-1";

/// Knows exactly one repo, `alice/demo`.
async fn fake_core() -> String {
    let app = axum::Router::new().route(
        "/_internal/repos/uid-for-name",
        axum::routing::get(
            |q: axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                match q.get("name").map(String::as_str) {
                    Some("alice/demo") => axum::Json(serde_json::json!({"uid": REPO_UID})).into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/_internal", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn as_alice(req: axum::http::request::Builder) -> axum::http::request::Builder {
    req.header("x-gitquarry-user", "alice")
        .header("x-gitquarry-user-uid", "alice-uid")
        .header("x-gitquarry-csrf", "tok")
}

async fn page(state: &Arc<FederationState>) -> String {
    let req = as_alice(Request::get("/_account/tokens")).body(Body::empty()).unwrap();
    send(&router(Arc::clone(state)), req).await.body
}

async fn post(state: &Arc<FederationState>, form: &str) -> String {
    let req = as_alice(Request::post("/_account/tokens"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .unwrap();
    send(&router(Arc::clone(state)), req).await.body
}

async fn seed_token(state: &FederationState, write: bool) {
    domain::token::create(
        state.backend.as_ref(),
        &state.cipher,
        "alice",
        "peer A",
        [Uid::from(REPO_UID.to_string())].into_iter().collect(),
        write,
        None,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn revoking_reports_whether_a_token_was_actually_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), &fake_core().await);
    let (token, secret) = domain::token::create(
        state.backend.as_ref(),
        &state.cipher,
        "alice",
        "peer A",
        [Uid::from(REPO_UID.to_string())].into_iter().collect(),
        false,
        None,
    )
    .await
    .unwrap()
    .unwrap();

    assert!(
        post(&state, "action=revoke&uid=no-such-uid")
            .await
            .contains("no such token.")
    );
    assert!(
        post(&state, &format!("action=revoke&uid={}", token.uid))
            .await
            .contains("revoked.")
    );
    assert!(
        access::resolve_by_token(state.backend.as_ref(), &state.cipher, &secret)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn is_refused_without_the_user_and_uid_core_forwards() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), &fake_core().await);
    let req = Request::get("/_account/tokens")
        .header("x-gitquarry-user", "alice")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        send(&router(Arc::clone(&state)), req).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn empty_state_shows_hints() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), &fake_core().await);
    let html = page(&state).await;
    assert!(html.contains("no tokens yet"));
    assert!(html.contains("no peers linked"));
}

#[tokio::test]
async fn read_only_and_read_write_tokens_render_distinct_scope_labels() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), &fake_core().await);
    seed_token(&state, false).await;
    seed_token(&state, true).await;
    let html = page(&state).await;
    assert!(html.contains("read-only"));
    assert!(html.contains("read+write"));
}

#[tokio::test]
async fn a_created_token_is_shown_once_and_resolves_to_its_owner() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), &fake_core().await);

    let html = post(&state, "action=create&label=peer+A&repos=alice%2Fdemo").await;
    assert!(html.contains("your new token"), "{html}");
    let secret = html
        .split("<code>")
        .nth(1)
        .and_then(|rest| rest.split("</code>").next())
        .unwrap()
        .to_string();
    assert!(secret.starts_with("token_"));

    let (owner, token) = access::resolve_by_token(state.backend.as_ref(), &state.cipher, &secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner, "alice");
    assert!(token.repos.contains(&Uid::from(REPO_UID.to_string())));
    assert!(
        !page(&state).await.contains(&secret),
        "the secret is only ever shown on creation"
    );
}

#[tokio::test]
async fn creating_a_token_for_an_unknown_repo_or_with_a_bad_label_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), &fake_core().await);

    let html = post(&state, "action=create&label=peer+A&repos=mallory%2Fsecret").await;
    assert!(
        html.contains("repo doesn&#39;t exist") || html.contains("repo doesn't exist"),
        "{html}"
    );
    let html = post(&state, "action=create&label=&repos=alice%2Fdemo").await;
    assert!(!html.contains("your new token"));
    assert!(
        domain::token::list_for_owner(state.backend.as_ref(), &state.cipher, "alice")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_linked_peer_offers_the_link_form() {
    let dir = tempfile::tempdir().unwrap();
    let state = federation_state(dir.path(), &fake_core().await);
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();
    let html = page(&state).await;
    assert!(html.contains(r#"value="link_peer""#));
    assert!(html.contains(r#"<option value="acme">acme</option>"#));
}
