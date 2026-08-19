use gitquarry_ci::core_client::CoreClient;
use gitquarry_ci::state::CiState;
use gitquarry_ci::web::router;
use s3db::{LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;

fn test_state(root: &std::path::Path) -> Arc<CiState> {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[61u8; s3db_crypto::SALT_LEN]).unwrap();
    Arc::new(CiState {
        backend: Arc::new(backend),
        cipher: Arc::new(cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
    })
}

#[tokio::test]
async fn health_endpoint_responds_ok() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(test_state(dir.path()));
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::get("/_internal/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn pending_runs_endpoint_requires_repo_uid() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(test_state(dir.path()));
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::get("/_internal/runs/pending")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pending_runs_endpoint_returns_empty_list_for_a_repo_with_no_runs() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(test_state(dir.path()));
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::get("/_internal/runs/pending?repo_uid=repo-1")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let runs: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(runs.is_empty());
}
