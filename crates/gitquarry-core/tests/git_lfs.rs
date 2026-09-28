use gitquarry_core::auth::entities::Permission;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::git::lfs::*;
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::remote::Remote;
use gitquarry_core::storage::uid::Uid;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use sha2::{Digest, Sha256};

fn test_remote(root: &std::path::Path) -> Remote {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[3u8; SALT_LEN]).unwrap());
    Remote::new(backend, "repos/team/proj".to_string(), cipher)
}

#[test]
fn peek_operation_reads_the_field_ignoring_the_rest() {
    assert_eq!(peek_operation(br#"{"operation":"upload","objects":[]}"#), "upload");
    assert_eq!(
        peek_operation(br#"{"operation":"download","objects":[],"ref":{"name":"x"}}"#),
        "download"
    );
}

#[test]
fn peek_operation_defaults_to_empty_on_malformed_body() {
    assert_eq!(peek_operation(b"not json"), "");
    assert_eq!(peek_operation(br#"{"objects":[]}"#), "");
}

#[test]
fn required_permission_maps_upload_to_developer_everything_else_to_reporter() {
    assert_eq!(required_permission("upload"), Permission::Developer);
    assert_eq!(required_permission("download"), Permission::Reporter);
    assert_eq!(required_permission(""), Permission::Reporter);
    assert_eq!(required_permission("bogus"), Permission::Reporter);
}

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn denial_response_uses_the_lfs_json_error_shape() {
    let resp = denial_response(gitquarry_core::git::http::Denial::Unauthorized);
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), BATCH_CONTENT_TYPE);
    assert!(resp.headers().get(header::WWW_AUTHENTICATE).is_some());
    let json = body_json(resp).await;
    assert!(json["message"].is_string());

    let resp = denial_response(gitquarry_core::git::http::Denial::Forbidden);
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), BATCH_CONTENT_TYPE);
}

#[tokio::test]
async fn download_object_404_uses_the_lfs_json_error_shape() {
    let dir = tempfile::tempdir().unwrap();
    let remote = test_remote(dir.path());
    let resp = download_object(&remote, &"b".repeat(64)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), BATCH_CONTENT_TYPE);
    let json = body_json(resp).await;
    assert!(json["message"].is_string());
}

#[tokio::test]
async fn upload_object_mismatch_uses_the_lfs_json_error_shape() {
    let dir = tempfile::tempdir().unwrap();
    let remote = test_remote(dir.path());
    let wrong_oid = "c".repeat(64);
    let req = axum::http::Request::builder()
        .body(Body::from(b"hello".to_vec()))
        .unwrap();
    let (resp, written) = upload_object(&remote, &wrong_oid, req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), BATCH_CONTENT_TYPE);
    assert_eq!(written, 0);
    let json = body_json(resp).await;
    assert!(json["message"].as_str().unwrap().contains("oid"));
}

#[tokio::test]
async fn batch_response_malformed_json_uses_the_lfs_json_error_shape() {
    let dir = tempfile::tempdir().unwrap();
    let remote = test_remote(dir.path());
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "gitquarry.example".parse().unwrap());
    let resp = batch_response(&remote, "team/proj", &headers, Bytes::from_static(b"not json"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), BATCH_CONTENT_TYPE);
    let json = body_json(resp).await;
    assert!(json["message"].is_string());
}

#[test]
fn base_url_prefers_forwarded_proto_over_plain_http() {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "gitquarry.example".parse().unwrap());
    assert_eq!(base_url(&headers).as_deref(), Some("http://gitquarry.example"));
    headers.insert("x-forwarded-proto", "https".parse().unwrap());
    assert_eq!(base_url(&headers).as_deref(), Some("https://gitquarry.example"));
}

#[test]
fn base_url_none_without_a_host_header() {
    assert_eq!(base_url(&HeaderMap::new()), None);
}

#[test]
fn encode_hex_matches_a_known_digest() {
    // sha256("") — a standard test vector, not derived from this code.
    let empty_digest = Sha256::digest(b"");
    assert_eq!(
        encode_hex(&empty_digest),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[tokio::test]
async fn batch_response_reports_missing_object_for_download_and_skips_upload_when_present() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let remote = test_remote(bucket_dir.path());
    let oid = "a".repeat(64);
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "gitquarry.example".parse().unwrap());

    // Download of an object that doesn't exist yet: no action, an error.
    let req = format!(r#"{{"operation":"download","objects":[{{"oid":"{oid}","size":5}}]}}"#);
    let resp = batch_response(&remote, "team/proj", &headers, Bytes::from(req))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["objects"][0]["error"]["code"], 404);
    assert!(parsed["objects"][0]["actions"].is_null());

    // Upload of the same, not-yet-existing object: an upload action.
    let req = format!(r#"{{"operation":"upload","objects":[{{"oid":"{oid}","size":5}}]}}"#);
    let resp = batch_response(&remote, "team/proj", &headers, Bytes::from(req))
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        parsed["objects"][0]["actions"]["upload"]["href"]
            .as_str()
            .unwrap()
            .ends_with(&format!("team/proj/info/lfs/objects/{oid}"))
    );

    // Now upload the bytes for real, then re-batch as "upload": no
    // action needed (already stored), matching the spec's convention.
    let src = bucket_dir.path().join("src.bin");
    tokio::fs::write(&src, b"hello").await.unwrap();
    remote.upload_lfs_object(&oid, &src).await.unwrap();
    let req = format!(r#"{{"operation":"upload","objects":[{{"oid":"{oid}","size":5}}]}}"#);
    let resp = batch_response(&remote, "team/proj", &headers, Bytes::from(req))
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(parsed["objects"][0]["actions"].is_null());

    // And "download" now succeeds with a download action.
    let req = format!(r#"{{"operation":"download","objects":[{{"oid":"{oid}","size":5}}]}}"#);
    let resp = batch_response(&remote, "team/proj", &headers, Bytes::from(req))
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        parsed["objects"][0]["actions"]["download"]["href"]
            .as_str()
            .unwrap()
            .ends_with(&format!("team/proj/info/lfs/objects/{oid}"))
    );
}

#[tokio::test]
async fn download_object_streams_back_the_exact_uploaded_bytes() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let remote = test_remote(bucket_dir.path());
    let bytes: Vec<u8> = (0..(200 * 1024)).map(|i| (i % 256) as u8).collect();
    let oid = encode_hex(&Sha256::digest(&bytes));

    let src = bucket_dir.path().join("src.bin");
    tokio::fs::write(&src, &bytes).await.unwrap();
    remote.upload_lfs_object(&oid, &src).await.unwrap();

    let resp = download_object(&remote, &oid).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_LENGTH).unwrap().to_str().unwrap(),
        bytes.len().to_string()
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), bytes.as_slice());
}

#[tokio::test]
async fn download_object_404s_when_missing() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let remote = test_remote(bucket_dir.path());
    let resp = download_object(&remote, &"b".repeat(64)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

fn body_request(bytes: Vec<u8>) -> Request {
    axum::http::Request::builder().body(Body::from(bytes)).unwrap()
}

#[tokio::test]
async fn upload_object_rejects_content_not_matching_the_declared_oid() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let remote = test_remote(bucket_dir.path());
    let wrong_oid = "c".repeat(64);
    let (resp, written) = upload_object(&remote, &wrong_oid, body_request(b"hello".to_vec()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(written, 0);
    assert!(!remote.lfs_object_exists(&wrong_oid).await.unwrap());
}

#[tokio::test]
async fn upload_object_stores_content_matching_its_own_hash() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let remote = test_remote(bucket_dir.path());
    let bytes = b"hello, lfs".to_vec();
    let oid = encode_hex(&Sha256::digest(&bytes));

    let (resp, written) = upload_object(&remote, &oid, body_request(bytes.clone())).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(written, bytes.len() as u64);
    assert!(remote.lfs_object_exists(&oid).await.unwrap());

    let dest = bucket_dir.path().join("out.bin");
    remote.download_lfs_object(&oid, &dest).await.unwrap();
    assert_eq!(tokio::fs::read(&dest).await.unwrap(), bytes);
}

fn test_backend_cipher(root: &std::path::Path) -> (Arc<dyn Backend>, Arc<Cipher>) {
    let backend: Arc<dyn Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[9u8; SALT_LEN]).unwrap());
    (backend, cipher)
}

#[tokio::test]
async fn create_lock_then_list_then_conflict_on_the_same_path() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let resp = create_lock_response(
        &backend,
        &cipher,
        &uid,
        "alice",
        Bytes::from_static(br#"{"path":"a.psd"}"#),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(created["lock"]["id"], "1");
    assert_eq!(created["lock"]["path"], "a.psd");
    assert_eq!(created["lock"]["owner"]["name"], "alice");

    let resp = list_locks_response(&backend, &cipher, &uid, "").await.unwrap();
    let listed = body_json(resp).await;
    assert_eq!(listed["locks"].as_array().unwrap().len(), 1);
    assert_eq!(listed["locks"][0]["path"], "a.psd");

    // Same path, different owner -- rejected with the existing lock's info.
    let resp = create_lock_response(
        &backend,
        &cipher,
        &uid,
        "bob",
        Bytes::from_static(br#"{"path":"a.psd"}"#),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let conflict = body_json(resp).await;
    assert_eq!(conflict["lock"]["owner"]["name"], "alice");
}

#[tokio::test]
async fn list_locks_response_filters_by_path_and_id() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    create_lock_response(
        &backend,
        &cipher,
        &uid,
        "alice",
        Bytes::from_static(br#"{"path":"a.psd"}"#),
    )
    .await
    .unwrap();
    create_lock_response(
        &backend,
        &cipher,
        &uid,
        "bob",
        Bytes::from_static(br#"{"path":"b.psd"}"#),
    )
    .await
    .unwrap();

    let resp = list_locks_response(&backend, &cipher, &uid, "path=b.psd")
        .await
        .unwrap();
    let listed = body_json(resp).await;
    assert_eq!(listed["locks"].as_array().unwrap().len(), 1);
    assert_eq!(listed["locks"][0]["path"], "b.psd");

    let resp = list_locks_response(&backend, &cipher, &uid, "id=1").await.unwrap();
    let listed = body_json(resp).await;
    assert_eq!(listed["locks"].as_array().unwrap().len(), 1);
    assert_eq!(listed["locks"][0]["path"], "a.psd");
}

#[tokio::test]
async fn verify_locks_response_splits_ours_and_theirs() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    create_lock_response(
        &backend,
        &cipher,
        &uid,
        "alice",
        Bytes::from_static(br#"{"path":"a.psd"}"#),
    )
    .await
    .unwrap();
    create_lock_response(
        &backend,
        &cipher,
        &uid,
        "bob",
        Bytes::from_static(br#"{"path":"b.psd"}"#),
    )
    .await
    .unwrap();

    let resp = verify_locks_response(&backend, &cipher, &uid, Some("alice"))
        .await
        .unwrap();
    let verified = body_json(resp).await;
    assert_eq!(verified["ours"].as_array().unwrap().len(), 1);
    assert_eq!(verified["ours"][0]["path"], "a.psd");
    assert_eq!(verified["theirs"].as_array().unwrap().len(), 1);
    assert_eq!(verified["theirs"][0]["path"], "b.psd");

    let resp = verify_locks_response(&backend, &cipher, &uid, None).await.unwrap();
    let verified = body_json(resp).await;
    assert_eq!(verified["ours"].as_array().unwrap().len(), 0);
    assert_eq!(verified["theirs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unlock_response_rejects_a_non_owner_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    create_lock_response(
        &backend,
        &cipher,
        &uid,
        "alice",
        Bytes::from_static(br#"{"path":"a.psd"}"#),
    )
    .await
    .unwrap();

    let resp = unlock_response(&backend, &cipher, &uid, "1", "bob", Bytes::from_static(b"{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let listed = list_locks_response(&backend, &cipher, &uid, "").await.unwrap();
    assert_eq!(body_json(listed).await["locks"].as_array().unwrap().len(), 1);

    // `force` lets a different user break it.
    let resp = unlock_response(
        &backend,
        &cipher,
        &uid,
        "1",
        "bob",
        Bytes::from_static(br#"{"force":true}"#),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = list_locks_response(&backend, &cipher, &uid, "").await.unwrap();
    assert_eq!(body_json(listed).await["locks"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn unlock_response_owner_can_unlock_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());
    create_lock_response(
        &backend,
        &cipher,
        &uid,
        "alice",
        Bytes::from_static(br#"{"path":"a.psd"}"#),
    )
    .await
    .unwrap();

    let resp = unlock_response(&backend, &cipher, &uid, "1", "alice", Bytes::from_static(b"{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn unlock_response_404s_for_a_nonexistent_lock() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = test_backend_cipher(dir.path());
    let uid = Uid::from("team/proj".to_string());

    let resp = unlock_response(&backend, &cipher, &uid, "999", "alice", Bytes::from_static(b"{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
