//! Git LFS: batch API (`POST .../info/lfs/objects/batch`, `upload`/
//! `download` operations, see
//! <https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md>) plus
//! direct object transfer (`GET`/`PUT .../info/lfs/objects/<oid>`) plus the
//! locking API (`GET`/`POST .../info/lfs/locks`, `POST .../locks/verify`,
//! `POST .../locks/<id>/unlock`, see
//! <https://github.com/git-lfs/git-lfs/blob/main/docs/api/locking.md>).
//! Objects are content-addressed by sha256 `oid`, stored via
//! [`crate::storage::remote::Remote`]'s LFS methods — same encrypted,
//! streaming storage packs already use. Locks are unrelated small JSON
//! metadata about a path (never a blob), so they go through
//! [`crate::storage::lfs_locks`] (an `s3db` collection) instead — the lock
//! functions below take `backend`/`cipher`/`uid` directly rather than a
//! `Remote`. Wired into [`super::http`], which owns routing/auth for both
//! this and plain git-smart-HTTP.

use crate::crypto::Cipher;
use crate::storage::Backend;
use crate::storage::lfs_locks::{self, CreatedLock, LfsLock};
use crate::storage::remote::Remote;
use crate::storage::uid::Uid;
use crate::validate;
use anyhow::{Context, Result};
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) const BATCH_CONTENT_TYPE: &str = "application/vnd.git-lfs+json";

#[derive(Deserialize)]
struct BatchRequest {
    operation: String,
    objects: Vec<BatchObjectReq>,
}

/// Just the `operation` field, ignoring everything else (serde ignores
/// unrecognized fields by default) — read once, before the whole request is
/// authorized, so [`required_permission`] knows whether this needs `Read` or
/// `Write` without fully parsing (and validating) the request twice. An
/// unparseable body reads as an empty operation, which needs only the
/// less-privileged `Read`:
/// [`batch_response`]'s own full parse rejects it with a clear error right
/// after, so this never grants more than the caller could get anyway.
#[derive(Deserialize)]
struct OperationOnly {
    operation: String,
}

pub(super) fn peek_operation(body: &[u8]) -> String {
    serde_json::from_slice::<OperationOnly>(body)
        .map(|o| o.operation)
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct BatchObjectReq {
    oid: String,
    size: u64,
}

#[derive(Serialize)]
struct BatchResponse {
    objects: Vec<BatchObjectResp>,
}

#[derive(Serialize)]
struct BatchObjectResp {
    oid: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    authenticated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actions: Option<Actions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ObjectError>,
}

#[derive(Serialize)]
struct Actions {
    #[serde(skip_serializing_if = "Option::is_none")]
    upload: Option<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    download: Option<Action>,
}

#[derive(Serialize)]
struct Action {
    href: String,
}

#[derive(Serialize)]
struct ObjectError {
    code: u16,
    message: String,
}

/// The top-level error shape the Git LFS spec defines for every non-2xx
/// response from any LFS endpoint (not just a per-object `error` field
/// inside an otherwise-200 batch reply — see
/// <https://github.com/git-lfs/git-lfs/blob/main/docs/api/README.md#error-responses>):
/// `{"message": "..."}`, `Content-Type: application/vnd.git-lfs+json`.
/// `documentation_url`/`request_id` are optional per spec and this server
/// has neither a docs site nor per-request tracing to offer, so they're
/// simply omitted rather than filled with a placeholder.
#[derive(Serialize)]
struct TopLevelError {
    message: String,
}

pub(super) fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = TopLevelError {
        message: message.into(),
    };
    // `serde_json::to_vec` on a struct this simple can't fail; falling back
    // to an empty JSON object rather than unwrapping keeps this function
    // infallible, matching every other response builder in this file.
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, BATCH_CONTENT_TYPE)], bytes).into_response()
}

/// Renders an [`super::http::Denial`] in the shape the Git LFS spec expects
/// — same status codes `git::http`'s own `unauthorized()`/`forbidden()`
/// use, but a JSON body instead of plain text, since an LFS client parses
/// the response body rather than just displaying it like the git CLI does
/// for plain git-smart-HTTP errors. Basic auth is still the credential here
/// (same as plain git-http), so a 401 still carries `WWW-Authenticate` —
/// LFS clients that support Basic auth look for it the same way git itself
/// does.
pub(super) fn denial_response(reason: super::http::Denial) -> Response {
    match reason {
        super::http::Denial::Unauthorized => {
            let mut resp = error_response(StatusCode::UNAUTHORIZED, "credentials required");
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"gitquarry\"".parse().expect("valid header"),
            );
            resp
        }
        super::http::Denial::Forbidden => error_response(StatusCode::FORBIDDEN, "no permission"),
    }
}

/// `"upload"` needs [`crate::auth::entities::Permission::Developer`],
/// `"download"` (or anything else the client sends — the spec only defines
/// these two) needs `Reporter`. Parsed once, before any object is touched,
/// so a malformed `operation` fails the whole batch with one clear error
/// rather than a confusing per-object one.
pub(super) fn required_permission(operation: &str) -> crate::auth::entities::Permission {
    use crate::auth::entities::Permission;
    if operation == "upload" {
        Permission::Developer
    } else {
        Permission::Reporter
    }
}

/// Base URL (scheme+host, no trailing slash) LFS action `href`s are built
/// against — derived from the request's own `Host` + `X-Forwarded-Proto`
/// headers, not a config field: this server is always meant to run behind a
/// reverse proxy (same assumption `oidc_redirect_base_url` makes explicit
/// for a different flow), and forwarding those headers is the standard way
/// such a proxy tells the origin what the client actually sees. Falls back
/// to `http://<host>` if the proxy doesn't forward the header — wrong
/// findable only by a client hitting this server directly over plain HTTP
/// through a proxy that terminates TLS without adding the header, at which
/// point the mismatch is visible immediately (LFS actions simply fail),
/// not a silent security issue.
fn base_url(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    Some(format!("{scheme}://{host}"))
}

pub(super) async fn batch_response(
    remote: &Remote,
    repo_path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let req: BatchRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return Ok(error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("invalid batch: {e}"),
            ));
        }
    };
    let Some(base) = base_url(headers) else {
        return Ok(error_response(StatusCode::UNPROCESSABLE_ENTITY, "missing Host header"));
    };
    let is_upload = req.operation == "upload";

    let mut objects = Vec::with_capacity(req.objects.len());
    for obj in req.objects {
        if validate::validate_lfs_oid(&obj.oid).is_err() {
            objects.push(BatchObjectResp {
                oid: obj.oid,
                size: obj.size,
                authenticated: None,
                actions: None,
                error: Some(ObjectError {
                    code: 422,
                    message: "invalid oid (expected sha256)".to_string(),
                }),
            });
            continue;
        }
        let href = format!("{base}/{repo_path}/info/lfs/objects/{}", obj.oid);
        let exists = remote.lfs_object_exists(&obj.oid).await?;
        let actions = if is_upload {
            // Already stored: nothing to do, matches the spec's "omit the
            // upload action when the object already exists" convention.
            (!exists).then_some(Actions {
                upload: Some(Action { href }),
                download: None,
            })
        } else if exists {
            Some(Actions {
                upload: None,
                download: Some(Action { href }),
            })
        } else {
            None
        };
        let error = (!is_upload && !exists).then_some(ObjectError {
            code: 404,
            message: "object doesn't exist".to_string(),
        });
        objects.push(BatchObjectResp {
            oid: obj.oid,
            size: obj.size,
            authenticated: Some(true),
            actions,
            error,
        });
    }

    let resp = BatchResponse { objects };
    let body = serde_json::to_vec(&resp).context("serializing LFS batch response")?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, BATCH_CONTENT_TYPE)], body).into_response())
}

/// Lowercase hex, no new dependency for what's a two-line loop — `hex` is
/// only in this binary transitively (via other crates' internals), not a
/// direct dependency this module can rely on staying available.
fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `PUT .../info/lfs/objects/<oid>`: streams the request body into a scratch
/// tempfile while hashing it, rejects on a sha256 mismatch against `oid`
/// (the integrity guarantee — declared `size` is never checked against the
/// actual byte count, same as a plain `git push` isn't capped either), then
/// uploads via [`Remote::upload_lfs_object`]. The `u64` half of the return
/// is how many bytes actually got written to storage — `0` on every
/// early-return error path — so the caller (`git::http`, which already
/// knows whether this `oid` was new) can bump `RepoConfig::lfs_bytes`
/// without this module needing an `AppContext`/uid of its own.
pub(super) async fn upload_object(remote: &Remote, oid: &str, req: Request) -> Result<(Response, u64)> {
    let tmp = tempfile::Builder::new()
        .prefix(".gitquarry-lfs-")
        .tempfile()
        .context("creating temp file for LFS upload")?
        .into_temp_path();

    let mut file = tokio::fs::File::create(&tmp).await.context("opening LFS temp file")?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut stream = req.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading LFS upload body")?;
        hasher.update(&chunk);
        written += chunk.len() as u64;
        file.write_all(&chunk).await.context("writing LFS temp file")?;
    }
    file.flush().await.context("flushing LFS temp file")?;
    drop(file);

    let digest = encode_hex(&hasher.finalize());
    if digest != oid {
        return Ok((
            error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("content doesn't match the declared oid (computed {digest})"),
            ),
            0,
        ));
    }

    remote.upload_lfs_object(oid, &tmp).await?;
    Ok((StatusCode::OK.into_response(), written))
}

/// `GET .../info/lfs/objects/<oid>`: downloads (decrypts) to a scratch
/// tempfile, then streams the plaintext back in fixed-size chunks —
/// `Content-Length` comes from measuring that already-decrypted file, not a
/// separate lookup (the backend only knows the ciphertext's length, which
/// includes encryption overhead — see [`Remote::lfs_object_exists`]'s doc
/// comment).
pub(super) async fn download_object(remote: &Remote, oid: &str) -> Result<Response> {
    if !remote.lfs_object_exists(oid).await? {
        return Ok(error_response(StatusCode::NOT_FOUND, "object doesn't exist"));
    }
    let tmp_dir = tempfile::tempdir().context("creating temp dir for LFS download")?;
    let tmp_path = tmp_dir.path().join(oid);
    remote.download_lfs_object(oid, &tmp_path).await?;

    let len = tokio::fs::metadata(&tmp_path)
        .await
        .context("measuring LFS temp file")?
        .len();
    let file = tokio::fs::File::open(&tmp_path)
        .await
        .context("opening LFS temp file for response")?;

    const CHUNK_LEN: usize = 64 * 1024;
    let body_stream = stream::try_unfold(
        (file, tmp_dir, vec![0u8; CHUNK_LEN]),
        |(mut f, guard, mut buf)| async move {
            let n = f.read(&mut buf).await?;
            if n == 0 {
                drop(guard); // deletes the scratch dir only once fully read
                return Ok::<_, std::io::Error>(None);
            }
            Ok(Some((Bytes::copy_from_slice(&buf[..n]), (f, guard, buf))))
        },
    );

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        Body::from_stream(body_stream),
    )
        .into_response())
}

#[derive(Serialize)]
struct LockOwnerJson {
    name: String,
}

/// The wire shape the locking API spec uses for a lock: `id` is a *string*
/// (unlike [`LfsLock::id`]'s own `u64`) and `owner` is a nested object, not
/// a bare username.
#[derive(Serialize)]
struct LockJson {
    id: String,
    path: String,
    locked_at: String,
    owner: LockOwnerJson,
}

impl From<&LfsLock> for LockJson {
    fn from(lock: &LfsLock) -> Self {
        LockJson {
            id: lock.id.to_string(),
            path: lock.path.clone(),
            locked_at: lock.locked_at.clone(),
            owner: LockOwnerJson {
                name: lock.owner.clone(),
            },
        }
    }
}

#[derive(Serialize)]
struct LockResponse {
    lock: LockJson,
}

#[derive(Serialize)]
struct LockConflictResponse {
    lock: LockJson,
    message: String,
}

#[derive(Serialize)]
struct ListLocksResponse {
    locks: Vec<LockJson>,
}

#[derive(Serialize)]
struct VerifyLocksResponse {
    ours: Vec<LockJson>,
    theirs: Vec<LockJson>,
}

#[derive(Deserialize)]
struct CreateLockRequest {
    path: String,
}

#[derive(Deserialize, Default)]
struct UnlockRequest {
    #[serde(default)]
    force: bool,
}

/// Tiny local copy of `git::http`'s own `query_param` — that one is
/// module-private, and duplicating five lines beats widening its visibility
/// just to reach across for this.
fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// `GET .../info/lfs/locks`: every current lock, optionally narrowed by the
/// spec's `path`/`id` query params (`cursor`/`limit`/`refspec` are accepted
/// by clients but ignored — there's no pagination to page through at the
/// scale a single repo's lock list actually reaches).
pub(super) async fn list_locks_response(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    query: &str,
) -> Result<Response> {
    let mut locks = lfs_locks::list_locks(backend, cipher, uid).await?;
    if let Some(path) = query_param(query, "path") {
        locks.retain(|l| l.path == path);
    }
    if let Some(id) = query_param(query, "id") {
        locks.retain(|l| l.id.to_string() == id);
    }
    let body = serde_json::to_vec(&ListLocksResponse {
        locks: locks.iter().map(LockJson::from).collect(),
    })
    .context("serializing lock list response")?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, BATCH_CONTENT_TYPE)], body).into_response())
}

/// `POST .../info/lfs/locks`: claims `path` for `owner` (the authenticated
/// username — lock creation always requires `Permission::Developer`, so
/// there's always a real credential behind it, never an anonymous caller).
/// 409 with the existing lock's own JSON if someone already holds it, per
/// spec — first claim wins, no queueing, no stacking a second lock on the
/// same path even for the same owner.
pub(super) async fn create_lock_response(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    owner: &str,
    body: Bytes,
) -> Result<Response> {
    let req: CreateLockRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return Ok(error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("invalid lock request: {e}"),
            ));
        }
    };
    if validate::validate_tree_path(&req.path).is_err() {
        return Ok(error_response(StatusCode::UNPROCESSABLE_ENTITY, "invalid path"));
    }

    // Still checked by listing, not only by the path claim, because locks
    // taken before path claims existed have no claim to collide with.
    let existing = lfs_locks::list_locks(backend, cipher, uid).await?;
    let held = match existing.into_iter().find(|l| l.path == req.path) {
        Some(held) => held,
        None => match lfs_locks::create_lock(
            backend.as_ref(),
            cipher.as_ref(),
            uid,
            req.path,
            owner.to_string(),
            crate::storage::audit::format_unix_secs(now_secs()),
        )
        .await?
        {
            CreatedLock::Held(held) => held,
            CreatedLock::Created(lock) => {
                let resp_body = serde_json::to_vec(&LockResponse {
                    lock: LockJson::from(&lock),
                })
                .context("serializing lock response")?;
                return Ok((
                    StatusCode::CREATED,
                    [(header::CONTENT_TYPE, BATCH_CONTENT_TYPE)],
                    resp_body,
                )
                    .into_response());
            }
        },
    };
    let body = serde_json::to_vec(&LockConflictResponse {
        lock: LockJson::from(&held),
        message: "already locked".to_string(),
    })
    .context("serializing lock conflict response")?;
    Ok((StatusCode::CONFLICT, [(header::CONTENT_TYPE, BATCH_CONTENT_TYPE)], body).into_response())
}

/// `POST .../info/lfs/locks/verify`: every current lock split into `ours`
/// (owned by `actor`) and `theirs` — `actor` is `None` only for an
/// anonymous caller on a `Public` repo (this endpoint only needs
/// `Permission::Reporter`), in which case everything is `theirs`.
pub(super) async fn verify_locks_response(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    actor: Option<&str>,
) -> Result<Response> {
    let locks = lfs_locks::list_locks(backend, cipher, uid).await?;
    let (ours, theirs): (Vec<&LfsLock>, Vec<&LfsLock>) = locks.iter().partition(|l| actor == Some(l.owner.as_str()));
    let body = serde_json::to_vec(&VerifyLocksResponse {
        ours: ours.into_iter().map(LockJson::from).collect(),
        theirs: theirs.into_iter().map(LockJson::from).collect(),
    })
    .context("serializing lock verify response")?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, BATCH_CONTENT_TYPE)], body).into_response())
}

/// `POST .../info/lfs/locks/<id>/unlock`: removes the lock if `requester`
/// owns it, or unconditionally when the request sets `force` — per spec,
/// breaking someone else's lock is allowed for anyone with push access
/// (this endpoint already requires `Permission::Developer`), not just the
/// repo owner.
pub(super) async fn unlock_response(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    uid: &Uid,
    id: &str,
    requester: &str,
    body: Bytes,
) -> Result<Response> {
    let Ok(id) = id.parse::<u64>() else {
        return Ok(error_response(StatusCode::NOT_FOUND, "lock doesn't exist"));
    };
    let req: UnlockRequest = if body.is_empty() {
        UnlockRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("invalid unlock request: {e}"),
                ));
            }
        }
    };
    let Some(lock) = lfs_locks::get_lock(backend.as_ref(), cipher.as_ref(), uid, id).await?.0 else {
        return Ok(error_response(StatusCode::NOT_FOUND, "lock doesn't exist"));
    };
    if lock.owner != requester && !req.force {
        return Ok(error_response(StatusCode::FORBIDDEN, "locked by another user"));
    }
    lfs_locks::delete_lock(backend.as_ref(), cipher.as_ref(), uid, &lock).await?;

    let resp_body = serde_json::to_vec(&LockResponse {
        lock: LockJson::from(&lock),
    })
    .context("serializing unlock response")?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, BATCH_CONTENT_TYPE)], resp_body).into_response())
}
