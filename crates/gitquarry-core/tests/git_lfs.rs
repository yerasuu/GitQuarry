//! Git LFS over the real git-smart-HTTP entry point: every request goes
//! through `git::http::middleware` on a real socket (it extracts a real
//! `ConnectInfo`), so routing, `authorize` and the LFS handlers are exercised
//! together, exactly as a `git lfs` client reaches them.

use axum::http::{StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use gitquarry_core::storage::settings;
use gitquarry_test_support::PASSWORD;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Mirrors `git::lfs::BATCH_CONTENT_TYPE`, hardcoded so the test pins the
/// wire value the Git LFS spec mandates rather than whatever the source says.
const LFS_JSON: &str = "application/vnd.git-lfs+json";

/// Private: every operation needs a credential.
const PRIVATE: &str = "team/proj";
/// Public: reads are open to anonymous callers, writes still need one.
const PUBLIC: &str = "team/open";

struct Server {
    base: String,
    addr: std::net::SocketAddr,
    ctx: Arc<AppContext>,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
}

async fn create_repo(ctx: &AppContext, full_name: &str, visibility: Visibility) {
    let resolved = ctx.create_repo(full_name).await.unwrap().unwrap();
    let config = RepoConfig {
        uid: resolved.uid.clone(),
        full_name: full_name.to_string(),
        visibility,
        ..Default::default()
    };
    settings::put_repo_config(
        ctx.backend().as_ref(),
        resolved.uid.as_str(),
        &config,
        None,
        &ctx.cipher,
    )
    .await
    .unwrap();
}

async fn create_user(ctx: &AppContext, username: &str, perm: Permission) {
    gitquarry_test_support::user(ctx, username, false).await;
    for repo in [PRIVATE, PUBLIC] {
        user::set_repo_perm(ctx, username, repo, Some(perm), gitquarry_i18n::Locale::En)
            .await
            .unwrap();
    }
}

/// alice and bob can push (`Developer`), carol can only read (`Reporter`).
async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    create_repo(&ctx, PRIVATE, Visibility::Private).await;
    create_repo(&ctx, PUBLIC, Visibility::Public).await;
    create_user(&ctx, "alice", Permission::Developer).await;
    create_user(&ctx, "bob", Permission::Developer).await;
    create_user(&ctx, "carol", Permission::Reporter).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // `.fallback()` before `.layer()`: a layer only wraps what the router
    // already has, so the middleware must come last to cover the fallback.
    let app =
        axum::Router::new()
            .fallback(|| async { StatusCode::NOT_FOUND })
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(&ctx),
                gitquarry_core::git::http::middleware,
            ));
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server {
        base: format!("http://{addr}"),
        addr,
        ctx,
        client: reqwest::Client::new(),
        _dir: dir,
    }
}

impl Server {
    fn url(&self, repo: &str, rest: &str) -> String {
        format!("{}/{repo}/info/lfs/{rest}", self.base)
    }

    fn post(
        &self,
        repo: &str,
        rest: &str,
        who: Option<&str>,
        body: impl Into<reqwest::Body>,
    ) -> reqwest::RequestBuilder {
        authed(self.client.post(self.url(repo, rest)), who)
            .header(header::CONTENT_TYPE, LFS_JSON)
            .body(body)
    }

    async fn batch(&self, repo: &str, who: Option<&str>, operation: &str, oid: &str, size: u64) -> reqwest::Response {
        let body = format!(r#"{{"operation":"{operation}","objects":[{{"oid":"{oid}","size":{size}}}]}}"#);
        self.post(repo, "objects/batch", who, body).send().await.unwrap()
    }

    async fn put_object(&self, repo: &str, who: Option<&str>, oid: &str, bytes: Vec<u8>) -> reqwest::Response {
        authed(self.client.put(self.url(repo, &format!("objects/{oid}"))), who)
            .body(bytes)
            .send()
            .await
            .unwrap()
    }

    async fn get_object(&self, repo: &str, who: Option<&str>, oid: &str) -> reqwest::Response {
        authed(self.client.get(self.url(repo, &format!("objects/{oid}"))), who)
            .send()
            .await
            .unwrap()
    }

    async fn create_lock(&self, repo: &str, who: &str, path: &str) -> reqwest::Response {
        self.post(repo, "locks", Some(who), format!(r#"{{"path":"{path}"}}"#))
            .send()
            .await
            .unwrap()
    }

    async fn list_locks(&self, repo: &str, who: Option<&str>, query: &str) -> reqwest::Response {
        authed(self.client.get(format!("{}{query}", self.url(repo, "locks"))), who)
            .send()
            .await
            .unwrap()
    }

    async fn unlock(&self, repo: &str, who: Option<&str>, id: &str, body: &'static str) -> reqwest::Response {
        self.post(repo, &format!("locks/{id}/unlock"), who, body)
            .send()
            .await
            .unwrap()
    }

    async fn repo_config(&self, repo: &str) -> RepoConfig {
        let resolved = self.ctx.resolve_repo(repo).await.unwrap().unwrap();
        resolved.remote.get_repo_config().await.unwrap().0
    }
}

fn authed(req: reqwest::RequestBuilder, who: Option<&str>) -> reqwest::RequestBuilder {
    match who {
        Some(username) => req.basic_auth(username, Some(PASSWORD)),
        None => req,
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn content_type(resp: &reqwest::Response) -> &str {
    resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap()
}

/// Asserts the spec's top-level error envelope (`{"message": ...}` as
/// `application/vnd.git-lfs+json`) that an LFS client parses on any non-2xx.
async fn assert_lfs_error(resp: reqwest::Response, status: StatusCode) -> String {
    assert_eq!(resp.status(), status);
    assert_eq!(content_type(&resp), LFS_JSON);
    let json: serde_json::Value = resp.json().await.unwrap();
    json["message"].as_str().unwrap().to_string()
}

async fn json(resp: reqwest::Response) -> serde_json::Value {
    assert!(resp.status().is_success(), "status {}", resp.status());
    assert_eq!(content_type(&resp), LFS_JSON);
    resp.json().await.unwrap()
}

async fn lock_count(server: &Server) -> usize {
    json(server.list_locks(PRIVATE, Some("alice"), "").await).await["locks"]
        .as_array()
        .unwrap()
        .len()
}

#[tokio::test]
async fn anonymous_on_a_private_repo_gets_a_401_lfs_error_with_a_basic_challenge() {
    let server = start().await;
    let oid = "a".repeat(64);
    let responses = [
        server.batch(PRIVATE, None, "download", &oid, 5).await,
        server.get_object(PRIVATE, None, &oid).await,
        server.list_locks(PRIVATE, None, "").await,
        server.post(PRIVATE, "locks/verify", None, "{}").send().await.unwrap(),
    ];
    for resp in responses {
        assert_eq!(
            resp.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Basic realm=\"gitquarry\""
        );
        assert_lfs_error(resp, StatusCode::UNAUTHORIZED).await;
    }
}

#[tokio::test]
async fn a_reader_can_download_but_every_write_is_a_403_lfs_error() {
    let server = start().await;
    let bytes = b"hello".to_vec();
    let oid = hex_sha256(&bytes);

    // Reads: batch download (and any operation that isn't "upload"),
    // object GET, lock list and verify only need `Reporter`.
    for operation in ["download", "", "bogus"] {
        let resp = server.batch(PRIVATE, Some("carol"), operation, &oid, 5).await;
        assert_eq!(resp.status(), StatusCode::OK, "operation {operation:?}");
    }
    assert_eq!(
        server.get_object(PRIVATE, Some("carol"), &oid).await.status(),
        StatusCode::NOT_FOUND
    );
    json(server.list_locks(PRIVATE, Some("carol"), "").await).await;
    json(
        server
            .post(PRIVATE, "locks/verify", Some("carol"), "{}")
            .send()
            .await
            .unwrap(),
    )
    .await;

    // Writes: batch upload, object PUT, lock create and unlock need `Developer`.
    assert_lfs_error(
        server.batch(PRIVATE, Some("carol"), "upload", &oid, 5).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_lfs_error(
        server.put_object(PRIVATE, Some("carol"), &oid, bytes).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_lfs_error(
        server.create_lock(PRIVATE, "carol", "a.psd").await,
        StatusCode::FORBIDDEN,
    )
    .await;
    json(server.create_lock(PRIVATE, "alice", "a.psd").await).await;
    assert_lfs_error(
        server.unlock(PRIVATE, Some("carol"), "1", r#"{"force":true}"#).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(lock_count(&server).await, 1);

    // The refused PUT stored nothing.
    let listed = json(server.batch(PRIVATE, Some("alice"), "download", &oid, 5).await).await;
    assert_eq!(listed["objects"][0]["error"]["code"], 404);
}

#[tokio::test]
async fn the_batch_operation_is_read_from_the_body_ignoring_unknown_fields() {
    let server = start().await;
    let oid = "a".repeat(64);
    let upload = format!(
        r#"{{"operation":"upload","ref":{{"name":"refs/heads/main"}},"objects":[{{"oid":"{oid}","size":5}}]}}"#
    );
    let resp = server
        .post(PRIVATE, "objects/batch", Some("carol"), upload.clone())
        .send()
        .await
        .unwrap();
    assert_lfs_error(resp, StatusCode::FORBIDDEN).await;
    let resp = server
        .post(PRIVATE, "objects/batch", Some("alice"), upload)
        .send()
        .await
        .unwrap();
    assert!(json(resp).await["objects"][0]["actions"]["upload"]["href"].is_string());

    let download = format!(r#"{{"operation":"download","ref":{{"name":"x"}},"objects":[{{"oid":"{oid}","size":5}}]}}"#);
    let resp = server
        .post(PRIVATE, "objects/batch", Some("carol"), download)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_malformed_batch_is_authorized_as_a_read_then_rejected_with_a_422_lfs_error() {
    let server = start().await;
    // A reader gets past authorization (an unparseable operation never
    // demands more than read) and then hits the full parse's 422, not a 403.
    for body in ["not json", r#"{"objects":[]}"#] {
        let resp = server
            .post(PRIVATE, "objects/batch", Some("carol"), body)
            .send()
            .await
            .unwrap();
        assert_lfs_error(resp, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
    // ...and on a private repo it still needs a credential at all.
    let resp = server
        .post(PRIVATE, "objects/batch", None, "not json")
        .send()
        .await
        .unwrap();
    assert_lfs_error(resp, StatusCode::UNAUTHORIZED).await;
}

#[tokio::test]
async fn anonymous_can_read_a_public_repo_but_writes_still_need_a_credential() {
    let server = start().await;
    let bytes = b"public blob".to_vec();
    let oid = hex_sha256(&bytes);
    assert_eq!(
        server
            .put_object(PUBLIC, Some("alice"), &oid, bytes.clone())
            .await
            .status(),
        StatusCode::OK
    );

    let resp = server.get_object(PUBLIC, None, &oid).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), bytes.as_slice());
    assert_eq!(
        server.batch(PUBLIC, None, "download", &oid, 11).await.status(),
        StatusCode::OK
    );
    json(server.list_locks(PUBLIC, None, "").await).await;

    assert_lfs_error(
        server.batch(PUBLIC, None, "upload", &oid, 11).await,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_lfs_error(
        server.put_object(PUBLIC, None, &oid, bytes).await,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_lfs_error(
        server
            .post(PUBLIC, "locks", None, r#"{"path":"a.psd"}"#)
            .send()
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_lfs_error(server.unlock(PUBLIC, None, "1", "{}").await, StatusCode::UNAUTHORIZED).await;
}

#[tokio::test]
async fn batch_reports_missing_objects_on_download_and_skips_upload_once_stored() {
    let server = start().await;
    let bytes = b"hello".to_vec();
    let oid = hex_sha256(&bytes);
    let href_suffix = format!("{PRIVATE}/info/lfs/objects/{oid}");

    // Download of an object that doesn't exist yet: no action, an error.
    let parsed = json(server.batch(PRIVATE, Some("alice"), "download", &oid, 5).await).await;
    assert_eq!(parsed["objects"][0]["error"]["code"], 404);
    assert!(parsed["objects"][0]["actions"].is_null());

    // Upload of the same, not-yet-existing object: an upload action.
    let parsed = json(server.batch(PRIVATE, Some("alice"), "upload", &oid, 5).await).await;
    let href = parsed["objects"][0]["actions"]["upload"]["href"].as_str().unwrap();
    assert!(href.ends_with(&href_suffix), "{href}");

    // Follow that action for real, then re-batch as "upload": no action
    // needed (already stored), matching the spec's convention.
    assert_eq!(
        server.put_object(PRIVATE, Some("alice"), &oid, bytes).await.status(),
        StatusCode::OK
    );
    let parsed = json(server.batch(PRIVATE, Some("alice"), "upload", &oid, 5).await).await;
    assert!(parsed["objects"][0]["actions"].is_null());
    assert!(parsed["objects"][0]["error"].is_null());

    // And "download" now succeeds with a download action.
    let parsed = json(server.batch(PRIVATE, Some("alice"), "download", &oid, 5).await).await;
    let href = parsed["objects"][0]["actions"]["download"]["href"].as_str().unwrap();
    assert!(href.ends_with(&href_suffix), "{href}");
}

#[tokio::test]
async fn batch_hrefs_follow_the_host_and_forwarded_proto_headers() {
    let server = start().await;
    let oid = "a".repeat(64);
    let body = format!(r#"{{"operation":"upload","objects":[{{"oid":"{oid}","size":5}}]}}"#);
    let href = |parsed: serde_json::Value| {
        parsed["objects"][0]["actions"]["upload"]["href"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let resp = server
        .post(PRIVATE, "objects/batch", Some("alice"), body.clone())
        .header(header::HOST, "gitquarry.example")
        .send()
        .await
        .unwrap();
    assert_eq!(
        href(json(resp).await),
        format!("http://gitquarry.example/{PRIVATE}/info/lfs/objects/{oid}")
    );

    let resp = server
        .post(PRIVATE, "objects/batch", Some("alice"), body)
        .header(header::HOST, "gitquarry.example")
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert_eq!(
        href(json(resp).await),
        format!("https://gitquarry.example/{PRIVATE}/info/lfs/objects/{oid}")
    );
}

/// HTTP/1.1 clients always send `Host`, so this speaks raw HTTP/1.0 to
/// reach the "no Host header" path: the batch can't build an `href` and
/// must refuse rather than hand out a relative or guessed URL.
#[tokio::test]
async fn batch_without_a_host_header_is_a_422() {
    let server = start().await;
    let body = r#"{"operation":"download","objects":[]}"#;
    let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let request = format!(
        "POST /{PUBLIC}/info/lfs/objects/batch HTTP/1.0\r\nContent-Type: {LFS_JSON}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status_line = response.lines().next().unwrap();
    assert!(status_line.contains(" 422 "), "{status_line}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains(&format!("content-type: {LFS_JSON}"))
    );
    assert!(response.contains("missing Host header"));
}

#[tokio::test]
async fn upload_then_download_round_trips_the_exact_bytes_across_chunks() {
    let server = start().await;
    // Larger than the 64 KiB download chunk, so the body spans several.
    let bytes: Vec<u8> = (0..(200 * 1024)).map(|i| (i % 256) as u8).collect();
    let oid = hex_sha256(&bytes);

    let resp = server.put_object(PRIVATE, Some("alice"), &oid, bytes.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = server.get_object(PRIVATE, Some("carol"), &oid).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_LENGTH).unwrap().to_str().unwrap(),
        bytes.len().to_string()
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), bytes.as_slice());

    // A new object's size is charged to the repo's LFS usage.
    assert_eq!(server.repo_config(PRIVATE).await.lfs_bytes, bytes.len() as u64);
}

#[tokio::test]
async fn the_oid_is_matched_as_lowercase_hex_sha256() {
    let server = start().await;
    // sha256("") — a standard test vector, not derived from this code.
    let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let resp = server.put_object(PRIVATE, Some("alice"), empty, Vec::new()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = server.get_object(PRIVATE, Some("alice"), empty).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn upload_rejects_content_not_matching_the_declared_oid() {
    let server = start().await;
    let wrong_oid = "c".repeat(64);
    let message = assert_lfs_error(
        server
            .put_object(PRIVATE, Some("alice"), &wrong_oid, b"hello".to_vec())
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    assert!(message.contains("oid"), "{message}");

    // Nothing stored and nothing charged.
    assert_lfs_error(
        server.get_object(PRIVATE, Some("alice"), &wrong_oid).await,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(server.repo_config(PRIVATE).await.lfs_bytes, 0);
}

#[tokio::test]
async fn download_of_a_missing_object_is_a_404_lfs_error() {
    let server = start().await;
    assert_lfs_error(
        server.get_object(PRIVATE, Some("alice"), &"b".repeat(64)).await,
        StatusCode::NOT_FOUND,
    )
    .await;
}

#[tokio::test]
async fn create_lock_then_list_then_conflict_on_the_same_path() {
    let server = start().await;

    let resp = server.create_lock(PRIVATE, "alice", "a.psd").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json(resp).await;
    assert_eq!(created["lock"]["id"], "1");
    assert_eq!(created["lock"]["path"], "a.psd");
    assert_eq!(created["lock"]["owner"]["name"], "alice");

    let listed = json(server.list_locks(PRIVATE, Some("carol"), "").await).await;
    assert_eq!(listed["locks"].as_array().unwrap().len(), 1);
    assert_eq!(listed["locks"][0]["path"], "a.psd");

    // Same path, different owner -- rejected with the existing lock's info.
    let resp = server.create_lock(PRIVATE, "bob", "a.psd").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(content_type(&resp), LFS_JSON);
    let conflict: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(conflict["lock"]["owner"]["name"], "alice");

    // Nor may the owner stack a second lock on it.
    assert_eq!(
        server.create_lock(PRIVATE, "alice", "a.psd").await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(lock_count(&server).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_on_one_path_grant_exactly_one_lock() {
    let server = start().await;
    let attempts =
        ["alice", "bob", "alice", "bob", "alice", "bob"].map(|who| server.create_lock(PRIVATE, who, "art/a.psd"));
    let statuses: Vec<_> = futures::future::join_all(attempts)
        .await
        .iter()
        .map(reqwest::Response::status)
        .collect();
    assert_eq!(statuses.iter().filter(|s| **s == StatusCode::CREATED).count(), 1);
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::CREATED || *s == StatusCode::CONFLICT)
    );
    assert_eq!(lock_count(&server).await, 1);

    // Unlocking frees the path for the next claim.
    let id = json(server.list_locks(PRIVATE, Some("alice"), "").await).await["locks"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = server
        .post(
            PRIVATE,
            &format!("locks/{id}/unlock"),
            Some("alice"),
            r#"{"force":true}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        server.create_lock(PRIVATE, "bob", "art/a.psd").await.status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn list_locks_filters_by_path_and_id() {
    let server = start().await;
    json(server.create_lock(PRIVATE, "alice", "a.psd").await).await;
    json(server.create_lock(PRIVATE, "bob", "b.psd").await).await;

    let listed = json(server.list_locks(PRIVATE, Some("carol"), "?path=b.psd").await).await;
    assert_eq!(listed["locks"].as_array().unwrap().len(), 1);
    assert_eq!(listed["locks"][0]["path"], "b.psd");

    let listed = json(server.list_locks(PRIVATE, Some("carol"), "?id=1").await).await;
    assert_eq!(listed["locks"].as_array().unwrap().len(), 1);
    assert_eq!(listed["locks"][0]["path"], "a.psd");
}

#[tokio::test]
async fn verify_locks_splits_ours_and_theirs_by_the_authenticated_user() {
    let server = start().await;
    for repo in [PRIVATE, PUBLIC] {
        json(server.create_lock(repo, "alice", "a.psd").await).await;
        json(server.create_lock(repo, "bob", "b.psd").await).await;
    }

    let verified = json(
        server
            .post(PRIVATE, "locks/verify", Some("alice"), "{}")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(verified["ours"].as_array().unwrap().len(), 1);
    assert_eq!(verified["ours"][0]["path"], "a.psd");
    assert_eq!(verified["theirs"].as_array().unwrap().len(), 1);
    assert_eq!(verified["theirs"][0]["path"], "b.psd");

    // An anonymous caller (only possible on a public repo) owns nothing.
    let verified = json(server.post(PUBLIC, "locks/verify", None, "{}").send().await.unwrap()).await;
    assert_eq!(verified["ours"].as_array().unwrap().len(), 0);
    assert_eq!(verified["theirs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unlock_rejects_a_non_owner_without_force() {
    let server = start().await;
    json(server.create_lock(PRIVATE, "alice", "a.psd").await).await;

    assert_lfs_error(
        server.unlock(PRIVATE, Some("bob"), "1", "{}").await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(lock_count(&server).await, 1);

    // `force` lets a different pusher break it.
    let unlocked = json(server.unlock(PRIVATE, Some("bob"), "1", r#"{"force":true}"#).await).await;
    assert_eq!(unlocked["lock"]["owner"]["name"], "alice");
    assert_eq!(lock_count(&server).await, 0);
}

#[tokio::test]
async fn unlock_by_the_owner_needs_no_force() {
    let server = start().await;
    json(server.create_lock(PRIVATE, "alice", "a.psd").await).await;

    let resp = server.unlock(PRIVATE, Some("alice"), "1", "{}").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(lock_count(&server).await, 0);
}

#[tokio::test]
async fn unlock_of_a_nonexistent_lock_is_a_404_lfs_error() {
    let server = start().await;
    for id in ["999", "not-a-number"] {
        assert_lfs_error(
            server.unlock(PRIVATE, Some("alice"), id, "{}").await,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
}

#[tokio::test]
async fn a_malformed_oid_or_an_oversize_body_is_the_clients_error() {
    let server = start().await;
    assert_lfs_error(
        server.get_object(PUBLIC, Some("alice"), "not-a-sha256").await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    // Mirrors `git::http::MAX_LFS_BATCH_BYTES`.
    let oversize = vec![b' '; 8 * 1024 * 1024 + 1];
    let resp = server
        .post(PUBLIC, "objects/batch", Some("alice"), oversize)
        .send()
        .await
        .unwrap();
    assert_lfs_error(resp, StatusCode::PAYLOAD_TOO_LARGE).await;
}

#[tokio::test]
async fn a_lock_that_fails_to_decrypt_fails_the_listing_instead_of_vanishing() {
    let server = start().await;
    json(server.create_lock(PRIVATE, "alice", "a.psd").await).await;
    let uid = server.ctx.resolve_repo(PRIVATE).await.unwrap().unwrap().uid;
    let key = gitquarry_core::storage::lfs_locks::lfs_locks_collection(&uid)
        .key("1")
        .unwrap();
    let backend = server.ctx.backend();
    let (_bytes, etag) = backend.get(&key).await.unwrap();
    backend
        .put_cas(&key, b"not a sealed object".to_vec(), etag.as_deref())
        .await
        .unwrap();

    let resp = server.list_locks(PRIVATE, Some("alice"), "").await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
