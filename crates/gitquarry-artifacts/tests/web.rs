use gitquarry_artifacts::core_client::CoreClient;
use gitquarry_artifacts::state::ArtifactsState;
use gitquarry_artifacts::web::router;
use s3db::{LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;

fn test_state(root: &std::path::Path) -> Arc<ArtifactsState> {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[101u8; s3db_crypto::SALT_LEN]).unwrap();
    Arc::new(ArtifactsState {
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
