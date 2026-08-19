use super::*;
use gitquarry_wire::{JobGraph, JobSpec};
use s3db::{LocalBackend, LocalEngine, LocalParams};

fn test_backend_and_cipher(root: &std::path::Path) -> (Arc<dyn Backend>, Arc<Cipher>) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[61u8; s3db_crypto::SALT_LEN]).unwrap();
    (Arc::new(backend), Arc::new(cipher))
}

fn job(id: &str, needs: Vec<String>) -> JobSpec {
    JobSpec {
        id: id.to_string(),
        image: "img".to_string(),
        commands: vec!["true".to_string()],
        needs,
        env: Default::default(),
        secrets: Vec::new(),
        artifacts: Vec::new(),
        kind: JobKind::Build,
    }
}

/// An invalid graph must be rejected before `ingest_graph` ever calls out
/// to core — proven here by pointing `CoreClient` at an unreachable
/// address: if validation ran second, this test would hang/error on the
/// network call instead of returning the validation error.
#[tokio::test]
async fn ingest_graph_rejects_an_invalid_graph_without_calling_core() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let core = CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string());

    let graph = JobGraph {
        schema_version: 1,
        jobs: vec![job("a", vec![]), job("a", vec![])],
    };

    let err = ingest_graph(&backend, &cipher, &core, "repo-1", "deadbeef", graph)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("duplicate job id"));
}
