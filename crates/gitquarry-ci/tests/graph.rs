use gitquarry_ci::core_client::CoreClient;
use gitquarry_ci::graph::{JobRunStatus, approve_run, ingest_graph, list_pending_approvals};
use gitquarry_wire::{JobGraph, JobKind, JobSpec};
use s3db::{LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;

fn test_backend_and_cipher(root: &std::path::Path) -> (Arc<dyn s3db::Backend>, Arc<s3db::Cipher>) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[61u8; s3db_crypto::SALT_LEN]).unwrap();
    (Arc::new(backend), Arc::new(cipher))
}

fn job(id: &str, kind: JobKind) -> JobSpec {
    JobSpec {
        id: id.to_string(),
        image: "img".to_string(),
        commands: vec!["true".to_string()],
        needs: Vec::new(),
        env: Default::default(),
        secrets: Vec::new(),
        artifacts: Vec::new(),
        kind,
    }
}

/// A tiny stand-in for `gitquarry-core`'s own `/_internal/ci/repo`,
/// answering with a fixed `ci_deploy_gate_disabled` value — enough to
/// drive `ingest_graph`'s gate decision without a real core process.
async fn fake_core(gate_disabled: bool) -> CoreClient {
    use axum::Router;
    use axum::extract::Query;
    use axum::routing::get;
    use std::collections::HashMap;

    async fn handler(
        Query(_params): Query<HashMap<String, String>>,
        gate_disabled: axum::extract::State<bool>,
    ) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "full_name": "acme/widgets",
            "default_branch": "main",
            "head_sha": "deadbeef",
            "ci_deploy_gate_disabled": *gate_disabled,
        }))
    }

    let app = Router::new()
        .route("/_internal/ci/repo", get(handler))
        .with_state(gate_disabled);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    CoreClient::new(format!("http://{addr}/_internal"), "unused".to_string())
}

#[tokio::test]
async fn deploy_job_lands_pending_approval_when_gate_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let core = fake_core(false).await;

    let graph = JobGraph {
        schema_version: 1,
        jobs: vec![job("build", JobKind::Build), job("deploy", JobKind::Deploy)],
    };
    let runs = ingest_graph(&backend, &cipher, &core, "repo-1", "sha1", graph)
        .await
        .unwrap();

    let build_run = runs.iter().find(|r| r.job_id == "build").unwrap();
    let deploy_run = runs.iter().find(|r| r.job_id == "deploy").unwrap();
    assert_eq!(build_run.status, JobRunStatus::Queued);
    assert_eq!(deploy_run.status, JobRunStatus::PendingApproval);
}

#[tokio::test]
async fn deploy_job_is_queued_directly_when_repo_disabled_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let core = fake_core(true).await;

    let graph = JobGraph {
        schema_version: 1,
        jobs: vec![job("deploy", JobKind::Deploy)],
    };
    let runs = ingest_graph(&backend, &cipher, &core, "repo-1", "sha1", graph)
        .await
        .unwrap();

    assert_eq!(runs[0].status, JobRunStatus::Queued);
}

#[tokio::test]
async fn approving_a_pending_run_moves_it_out_of_the_pending_list() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let core = fake_core(false).await;

    let graph = JobGraph {
        schema_version: 1,
        jobs: vec![job("deploy", JobKind::Deploy)],
    };
    let runs = ingest_graph(&backend, &cipher, &core, "repo-1", "sha1", graph)
        .await
        .unwrap();
    let run_id = &runs[0].id;

    let pending = list_pending_approvals(&backend, &cipher, "repo-1").await.unwrap();
    assert_eq!(pending.len(), 1);

    let approved = approve_run(&backend, &cipher, "repo-1", run_id).await.unwrap();
    assert!(approved);

    let pending = list_pending_approvals(&backend, &cipher, "repo-1").await.unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn approving_a_run_that_is_not_pending_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());
    let core = fake_core(false).await;

    let graph = JobGraph {
        schema_version: 1,
        jobs: vec![job("build", JobKind::Build)],
    };
    let runs = ingest_graph(&backend, &cipher, &core, "repo-1", "sha1", graph)
        .await
        .unwrap();

    let err = approve_run(&backend, &cipher, "repo-1", &runs[0].id).await.unwrap_err();
    assert!(err.to_string().contains("not pending approval"));
}

#[tokio::test]
async fn approving_an_unknown_run_returns_false() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_and_cipher(dir.path());

    let approved = approve_run(&backend, &cipher, "repo-1", "does-not-exist")
        .await
        .unwrap();
    assert!(!approved);
}
