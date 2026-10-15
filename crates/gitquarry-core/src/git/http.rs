//! git smart-HTTP: `GET .../info/refs?service=git-upload-pack|git-receive-pack`,
//! `POST .../git-upload-pack`, `POST .../git-receive-pack` — the protocol a
//! plain `git clone`/`push http://...` speaks, see
//! <https://git-scm.com/docs/http-protocol>. One of the two publicly
//! supported transports (alongside [`super::ssh`]); this file owns the wire
//! protocol only — it never renders HTML and never calls into
//! `gitquarry_webui::webui::repo`/`gitquarry_webui::webui::repo_settings`, and vice versa. It
//! has to live alongside `gitquarry_webui::webui`'s router rather than owning its own
//! listener (unlike [`super::ssh`]) because both transports are meant to
//! share one HTTP port — so it's wired in as [`middleware`], a layer over
//! the whole router (see `gitquarry_webui::webui::run_server`): it short-circuits the
//! three paths above before the browsing-UI's `/{*repo}` catch-all route
//! ever sees them, and passes everything else through unchanged — same
//! shape as `webui`'s own `security_headers` layer.
//!
//! Repo names can contain `/`, so (unlike a `{owner}/{repo}/info/refs`-style
//! route some forges use) there's no way to express "wildcard path, then a
//! literal suffix" as a router pattern; the suffix is stripped procedurally
//! in [`classify`] instead.

use crate::app::{AppContext, Mirror};
use crate::auth::access;
use crate::auth::entities::{Permission, User};
use crate::mirror;
use crate::storage::entities::Visibility;
use crate::validate;
use anyhow::{Context, Result, bail};
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use gitquarry_auth_crypto::password;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tracing::{debug, error, warn};

/// Request bodies for `POST .../info/lfs/objects/batch` only ever list
/// oids/sizes, never file content — a generous cap, well above any real
/// batch, just to keep a malformed/hostile request from buffering unbounded
/// memory before it's even parsed.
const MAX_LFS_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// `receive-pack` request bodies are real pack data, so this stays generous
/// enough to never trip on a legitimate push (well past any real monorepo
/// pack) -- it exists only so a runaway or hostile client can't fill the
/// disk [`spool_body`] writes it to before `git` ever sees a byte of it.
const MAX_PACK_BODY_BYTES: usize = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    InfoRefs,
    UploadPack,
    ReceivePack,
    LfsBatch,
    LfsObject,
    LfsLocks,
    LfsLocksVerify,
    LfsLockUnlock,
}

impl Kind {
    fn valid_method(self, method: &Method) -> bool {
        match self {
            Kind::InfoRefs => method == Method::GET,
            Kind::UploadPack | Kind::ReceivePack | Kind::LfsBatch => method == Method::POST,
            Kind::LfsObject => method == Method::GET || method == Method::PUT,
            Kind::LfsLocks => method == Method::GET || method == Method::POST,
            Kind::LfsLocksVerify | Kind::LfsLockUnlock => method == Method::POST,
        }
    }
}

const LFS_OBJECTS_INFIX: &str = "/info/lfs/objects/";
const LFS_LOCKS_INFIX: &str = "/info/lfs/locks/";

/// Strips a known git-smart-HTTP or Git LFS suffix off `path`, returning the
/// repo name (not yet sanitized), which endpoint it is, and (for
/// [`Kind::LfsObject`] only) the requested oid — `None` for every other
/// path, which the browsing UI handles as before.
fn classify(path: &str) -> Option<(&str, Kind, Option<&str>)> {
    for (suffix, kind) in [
        ("/info/refs", Kind::InfoRefs),
        ("/git-upload-pack", Kind::UploadPack),
        ("/git-receive-pack", Kind::ReceivePack),
        ("/info/lfs/objects/batch", Kind::LfsBatch),
        ("/info/lfs/locks/verify", Kind::LfsLocksVerify),
        ("/info/lfs/locks", Kind::LfsLocks),
    ] {
        if let Some(rest) = path.strip_suffix(suffix) {
            let repo = rest.strip_prefix('/').unwrap_or(rest);
            if !repo.is_empty() {
                return Some((repo, kind, None));
            }
        }
    }
    // The oid/lock-id is a variable trailing segment, so it can't join the
    // `strip_suffix` loop above — found via the last occurrence of the
    // fixed infix instead (a repo name containing this exact substring as a
    // real path segment is an accepted, harmless ambiguity — same class
    // this module's own doc comment already calls out for the git suffixes).
    if let Some((repo, oid)) = extract_variable_segment(path, LFS_OBJECTS_INFIX, "") {
        return Some((repo, Kind::LfsObject, Some(oid)));
    }
    if let Some((repo, id)) = extract_variable_segment(path, LFS_LOCKS_INFIX, "/unlock") {
        return Some((repo, Kind::LfsLockUnlock, Some(id)));
    }
    None
}

/// Shared by the two variable-trailing-segment cases above: `infix`'s last
/// occurrence splits off the repo name, and whatever's left after it (minus
/// `trailing`, stripped from the end if non-empty) is the segment — rejected
/// if empty or itself contains a `/` (would mean a further, unexpected path
/// segment, not a bare id).
fn extract_variable_segment<'a>(path: &'a str, infix: &str, trailing: &str) -> Option<(&'a str, &'a str)> {
    let idx = path.rfind(infix)?;
    let (head, tail) = path.split_at(idx);
    let after_infix = &tail[infix.len()..];
    let var = if trailing.is_empty() {
        after_infix
    } else {
        after_infix.strip_suffix(trailing)?
    };
    if var.is_empty() || var.contains('/') {
        return None;
    }
    let repo = head.strip_prefix('/').unwrap_or(head);
    (!repo.is_empty()).then_some((repo, var))
}

/// What the anonymous rate limiter counts against: the socket address, or
/// — only when the instance says a proxy owns the header
/// ([`AppContext::trust_forwarded_for`]) — the last `X-Forwarded-For` hop.
///
/// Behind a reverse proxy the socket address is the proxy's, so every
/// anonymous client in the world shares one bucket and one of them can
/// spend it for everybody. Trusting the header without a proxy that
/// overwrites it is the opposite failure: each request picks its own
/// bucket, and the limit stops existing. Neither is safe to guess, hence
/// the setting.
fn client_key(ctx: &AppContext, headers: &HeaderMap, socket_ip: std::net::IpAddr) -> String {
    if !ctx.trust_forwarded_for {
        return socket_ip.to_string();
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        // The last entry, because it is the one the trusted proxy appended
        // from its own socket; everything before it arrived in the client's
        // request and is whatever the client chose to send.
        .and_then(|v| v.rsplit(',').next())
        .map(str::trim)
        .filter(|hop| !hop.is_empty())
        .map_or_else(|| socket_ip.to_string(), str::to_string)
}

pub async fn middleware(
    State(ctx): State<Arc<AppContext>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let Some((repo, kind, oid)) = classify(req.uri().path()) else {
        return next.run(req).await;
    };
    if !kind.valid_method(req.method()) {
        return next.run(req).await;
    }
    // A suffix match alone doesn't make it git's: the browsing UI has its
    // own paths that can end the same way (a file named `info/refs` under
    // `/owner/repo/blob/main/`), and those must still reach it. Only a
    // prefix that names a real repo is taken here.
    let Ok(name) = validate::sanitize_path(repo) else {
        return next.run(req).await;
    };
    let resolved = match ctx.resolve_repo(&name).await {
        Ok(Some(resolved)) => resolved,
        Ok(None) => return next.run(req).await,
        Err(e) => {
            error!(error = %e, "git-http repo lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    let oid = oid.map(str::to_string);
    let client = client_key(&ctx, req.headers(), addr.ip());
    match handle(&ctx, &name, resolved, kind, oid.as_deref(), client, req).await {
        Ok(resp) => resp,
        Err(e) => match e.downcast_ref::<ClientError>() {
            Some(client_error) => client_error.response(kind),
            None => {
                error!(error = %e, "git-http request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        },
    }
}

/// A request the client got wrong, kept apart from server failures so it
/// is answered with its own 4xx instead of a 500 and an error log line
/// nobody on the server side can act on.
#[derive(Debug)]
enum ClientError {
    BadRequest(&'static str),
    TooLarge,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::BadRequest(reason) => f.write_str(reason),
            ClientError::TooLarge => f.write_str("request body too large"),
        }
    }
}

impl std::error::Error for ClientError {}

impl ClientError {
    /// LFS clients parse the error body, git only displays it, hence the
    /// two shapes.
    fn response(&self, kind: Kind) -> Response {
        let status = match self {
            ClientError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ClientError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        };
        match kind {
            Kind::InfoRefs | Kind::UploadPack | Kind::ReceivePack => (status, self.to_string()).into_response(),
            Kind::LfsBatch | Kind::LfsObject | Kind::LfsLocks | Kind::LfsLocksVerify | Kind::LfsLockUnlock => {
                crate::git::lfs::error_response(status, self.to_string())
            }
        }
    }
}

/// Buffers a request body up to `limit`, answering 413 past it.
async fn read_capped(body: axum::body::Body, limit: usize, what: &'static str) -> Result<Bytes> {
    axum::body::to_bytes(body, limit).await.map_err(|e| {
        let over_limit = std::iter::successors(Some(&e as &(dyn std::error::Error + 'static)), |err| err.source())
            .any(|err| err.is::<http_body_util::LengthLimitError>());
        if over_limit {
            ClientError::TooLarge.into()
        } else {
            anyhow::Error::new(e).context(what)
        }
    })
}

/// The `git <subcommand>` an `info/refs?service=` query asks to advertise.
fn advertised_subcommand(query: Option<&str>) -> Result<&'static str, ClientError> {
    match query_param(query.unwrap_or(""), "service") {
        Some("git-upload-pack") => Ok("upload-pack"),
        Some("git-receive-pack") => Ok("receive-pack"),
        Some(_) => Err(ClientError::BadRequest("unsupported service")),
        None => Err(ClientError::BadRequest("missing ?service=")),
    }
}

async fn handle(
    ctx: &Arc<AppContext>,
    name: &str,
    resolved: crate::app::ResolvedRepo,
    kind: Kind,
    oid: Option<&str>,
    client_key: String,
    req: Request,
) -> Result<Response> {
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    // A soft-deleted repo (`domain::repo::delete`) is invisible over every
    // transport until restored or purged, not just blocked on push like an
    // archived one — same treatment as a repo that never existed.
    if config.deleted_at.is_some() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    match kind {
        Kind::InfoRefs | Kind::UploadPack | Kind::ReceivePack => {
            let advertised = match kind {
                Kind::InfoRefs => Some(advertised_subcommand(req.uri().query())?),
                _ => None,
            };
            let required = if kind == Kind::ReceivePack {
                Permission::Developer
            } else {
                Permission::Reporter
            };
            let actor = match authorize(ctx, req.headers(), name, &resolved.uid, config.visibility, required).await? {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(reason.into_git_response()),
            };
            // A push opens with `GET info/refs?service=git-receive-pack`, so
            // that advertisement belongs to the push as much as the POST
            // after it does and gets the same treatment below.
            let is_push = kind == Kind::ReceivePack || advertised == Some("receive-pack");
            // Clone/fetch flood + scraping guard (`GitRateLimiter`'s own doc
            // comment) — a push is deliberately not covered, same
            // "read-heavy scraping is the actual threat model here" scoping
            // that doc comment describes.
            if !is_push {
                let under_limit = match &actor {
                    Some(username) => ctx.git_rate_limiter.check_authenticated(username),
                    None => ctx.git_rate_limiter.check_anonymous(&client_key),
                };
                if !under_limit {
                    return Ok(too_many_requests());
                }
            }
            // Reads only. Syncing right before a push is actively harmful:
            // a pull that lands first moves the branch, and the pre-receive
            // hook then rejects the client's own push as stale ("changed in
            // S3 since the last fetch") — a sync nobody asked for turning a
            // valid push into a failure. The push's own refs reach the
            // external side on the next sweep either way.
            if !is_push {
                crate::mirror::sync_external_before_serving(ctx, &resolved).await;
            }
            // A fetch only reads the mirror, so it takes the repo's lock
            // shared and stops queueing behind other fetches and web views
            // of the same repo; `receive-pack` writes it and still needs
            // the mirror to itself.
            match kind {
                Kind::InfoRefs => {
                    let mirror = ctx.open_mirror_read(&resolved).await?;
                    let git_subcmd = advertised.context("info/refs without a parsed service")?;
                    info_refs_response(ctx, name, &resolved.uid, &mirror, git_subcmd, actor.as_deref()).await
                }
                Kind::UploadPack => {
                    let body = PackBody::from_request(req)?;
                    let mirror = ctx.open_mirror_read(&resolved).await?;
                    upload_pack_response(ctx, name, &resolved.uid, mirror, body, actor.as_deref())
                }
                Kind::ReceivePack => {
                    // Spooled before the exclusive lock is taken, so a slow
                    // uploader holds up nobody else's push, fetch or page
                    // view of this repo while its pack trickles in.
                    let pack = spool_body(ctx, PackBody::from_request(req)?).await?;
                    let mirror = ctx.open_mirror(&resolved).await?;
                    // Snapshot before the push so the post-push block can
                    // diff against it — see `events::diff_refs`'s own doc
                    // comment for why this process has no other way to
                    // learn which refs a push touched.
                    let before_refs = crate::events::mirror_refs(&mirror.path).await;
                    let resp = receive_pack_response(ctx, name, &resolved.uid, &mirror, pack, actor.as_deref()).await?;
                    // Taken before the re-sync below, which reads the same
                    // possibly-stale manifest and could only blur the diff.
                    let after_refs = crate::events::mirror_refs(&mirror.path).await;
                    // The pre-receive hook already pushed to the bucket; this just
                    // refreshes the local mirror for the next reader, same as
                    // `ssh.rs` does after a push.
                    if let Err(e) = mirror::sync_bare_repo_from_s3(&mirror.remote, &mirror.path).await {
                        warn!(error = %e, "http: couldn't re-sync after push");
                    }
                    // Detached, same reasoning as `ssh.rs`'s own call to
                    // this: `domain::repo::update_primary_language` takes
                    // its own fresh lock rather than reusing `mirror`'s,
                    // which is about to be dropped at the end of this
                    // request. `update_search_index` shares the same
                    // reasoning and the same detached task.
                    let language_ctx = Arc::clone(ctx);
                    let language_uid = resolved.uid.clone();
                    let language_path = mirror.path.clone();
                    tokio::spawn(async move {
                        crate::domain::repo::update_primary_language(&language_ctx, &language_uid, &language_path)
                            .await;
                        crate::domain::repo::update_search_index(&language_ctx, &language_uid, &language_path).await;
                    });
                    // Detached, same reasoning as the language/search-index
                    // updates above: webhook/CI delivery must never make
                    // the pusher wait.
                    if let (Some(before_refs), Some(after_refs)) = (before_refs, after_refs) {
                        let refs = crate::events::diff_refs(&before_refs, &after_refs);
                        if !refs.is_empty() {
                            let event_ctx = Arc::clone(ctx);
                            let event = crate::events::RepoEvent::Push {
                                repo_uid: resolved.uid.clone(),
                                refs,
                                pusher: actor.clone(),
                            };
                            tokio::spawn(async move { crate::events::emit(&event_ctx, event) });
                        }
                    }
                    Ok(resp)
                }
                Kind::LfsBatch | Kind::LfsObject | Kind::LfsLocks | Kind::LfsLocksVerify | Kind::LfsLockUnlock => {
                    unreachable!("handled in the other match arm")
                }
            }
        }
        Kind::LfsBatch => {
            let headers = req.headers().clone();
            let body = read_capped(req.into_body(), MAX_LFS_BATCH_BYTES, "reading LFS batch body").await?;
            let required = crate::git::lfs::required_permission(&crate::git::lfs::peek_operation(&body));
            let actor = match authorize(ctx, &headers, name, &resolved.uid, config.visibility, required).await? {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(crate::git::lfs::denial_response(reason)),
            };
            let _ = actor; // not attributed anywhere for LFS (no git hook runs for this path)
            crate::git::lfs::batch_response(&resolved.remote, name, &headers, body).await
        }
        Kind::LfsObject => {
            let oid = oid.context("missing LFS oid")?;
            validate::validate_lfs_oid(oid).map_err(|_| ClientError::BadRequest("invalid LFS oid"))?;
            let is_upload = req.method() == Method::PUT;
            let required = if is_upload {
                Permission::Developer
            } else {
                Permission::Reporter
            };
            let actor = match authorize(ctx, req.headers(), name, &resolved.uid, config.visibility, required).await? {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(crate::git::lfs::denial_response(reason)),
            };
            let _ = actor;
            if is_upload {
                // The pre-receive hook already blocks pushes to an archived
                // repo, but LFS uploads never go through that hook (no git
                // subprocess is spawned for them) — without this check an
                // archived repo would silently keep accepting new LFS blobs
                // while ordinary pushes are rejected.
                if config.archived {
                    return Ok(crate::git::lfs::denial_response(Denial::Forbidden));
                }
                // Same reasoning as the archived check above, for the
                // owner's storage quota — `mirror::hook` covers ordinary
                // pushes, LFS uploads need their own gate.
                if crate::auth::access::check_quota(ctx.backend().as_ref(), &ctx.cipher, config.owner.as_ref())
                    .await?
                    .is_some()
                {
                    return Ok(crate::git::lfs::denial_response(Denial::Forbidden));
                }
                // Only bump the owner's usage for a genuinely new object —
                // a client re-uploading a hash it (or someone else) already
                // pushed elsewhere in this repo must never double-count.
                let already_had_it = resolved.remote.lfs_object_exists(oid).await?;
                let (resp, written) = crate::git::lfs::upload_object(&resolved.remote, oid, req).await?;
                if !already_had_it && written > 0 {
                    crate::storage::settings::bump_lfs_bytes(
                        ctx.backend().as_ref(),
                        &ctx.cipher,
                        &resolved.uid,
                        written,
                    )
                    .await;
                }
                Ok(resp)
            } else {
                crate::git::lfs::download_object(&resolved.remote, oid).await
            }
        }
        Kind::LfsLocks => {
            let is_create = req.method() == Method::POST;
            let required = if is_create {
                Permission::Developer
            } else {
                Permission::Reporter
            };
            let headers = req.headers().clone();
            let actor = match authorize(ctx, &headers, name, &resolved.uid, config.visibility, required).await? {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(crate::git::lfs::denial_response(reason)),
            };
            let backend = ctx.backend();
            if is_create {
                // Same reasoning as the LFS object upload arm above: the
                // pre-receive hook never sees this request, so an archived
                // repo needs its own check here too.
                if config.archived {
                    return Ok(crate::git::lfs::denial_response(Denial::Forbidden));
                }
                let owner = actor.context("authorize() granted Developer without a credential")?;
                let body = read_capped(req.into_body(), MAX_LFS_BATCH_BYTES, "reading LFS lock create body").await?;
                crate::git::lfs::create_lock_response(&backend, &ctx.cipher, &resolved.uid, &owner, body).await
            } else {
                let query = req.uri().query().unwrap_or("");
                crate::git::lfs::list_locks_response(&backend, &ctx.cipher, &resolved.uid, query).await
            }
        }
        Kind::LfsLocksVerify => {
            let actor = match authorize(
                ctx,
                req.headers(),
                name,
                &resolved.uid,
                config.visibility,
                Permission::Reporter,
            )
            .await?
            {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(crate::git::lfs::denial_response(reason)),
            };
            let backend = ctx.backend();
            crate::git::lfs::verify_locks_response(&backend, &ctx.cipher, &resolved.uid, actor.as_deref()).await
        }
        Kind::LfsLockUnlock => {
            let id = oid.context("missing lock id")?;
            let actor = match authorize(
                ctx,
                req.headers(),
                name,
                &resolved.uid,
                config.visibility,
                Permission::Developer,
            )
            .await?
            {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(crate::git::lfs::denial_response(reason)),
            };
            if config.archived {
                return Ok(crate::git::lfs::denial_response(Denial::Forbidden));
            }
            let requester = actor.context("authorize() granted Developer without a credential")?;
            let backend = ctx.backend();
            let body = read_capped(req.into_body(), MAX_LFS_BATCH_BYTES, "reading LFS unlock body").await?;
            crate::git::lfs::unlock_response(&backend, &ctx.cipher, &resolved.uid, id, &requester, body).await
        }
    }
}

#[derive(Debug)]
enum Authz {
    Allowed(Option<String>),
    Denied(Denial),
}

/// Which of the two ways `authorize()` can refuse a request — carried as a
/// plain reason, not a pre-built [`Response`], so each caller can render it
/// in whatever shape its own protocol expects: plain-text + a bare status
/// for git-smart-HTTP (git doesn't parse the body), a
/// `application/vnd.git-lfs+json` `{"message": "..."}` envelope for the LFS
/// endpoints (its spec expects exactly that shape on every error response,
/// not just inside a 200 batch reply's per-object `error` fields — see
/// [`super::lfs::denial_response`]).
#[derive(Debug)]
pub(super) enum Denial {
    Unauthorized,
    Forbidden,
}

impl Denial {
    /// git-smart-HTTP's own rendering — unchanged from before this type
    /// existed, just moved out of `authorize()` so it's no longer the only
    /// possible shape a denial can take.
    fn into_git_response(self) -> Response {
        match self {
            Denial::Unauthorized => unauthorized(),
            Denial::Forbidden => forbidden(),
        }
    }
}

/// Same gate `ssh.rs` applies, just over HTTP Basic instead of a pubkey: a
/// `Public` repo's read side stays reachable with no credentials at all
/// (matching the web UI's anonymous browsing); everything else — any write
/// (a push, or an LFS upload), or reading a `Limited`/`Private` repo — needs
/// either a valid `username`/`password_hash` pair (the same credential the
/// web login form uses) or a fine-grained token (federation layer 4,
/// `docs/federation-concept.md`'s "Capa 4") naming this exact repo — and at
/// least `required` either way.
///
/// The token path needs `ctx.federation` configured (`AccessToken` storage
/// moved to `gitquarry-federation` — `docs/federation-binary-split-concept.md`
/// section 2): without it, a token-shaped `username` just falls through to
/// the password path and fails there, same as before federation existed at
/// all.
async fn authorize(
    ctx: &AppContext,
    headers: &HeaderMap,
    name: &str,
    repo_uid: &crate::storage::uid::Uid,
    visibility: Visibility,
    required: Permission,
) -> Result<Authz> {
    if required == Permission::Reporter && visibility == Visibility::Public {
        return Ok(Authz::Allowed(None));
    }
    let Some(credential) = authenticate(ctx, headers, repo_uid).await? else {
        return Ok(Authz::Denied(Denial::Unauthorized));
    };
    let (username, perm) = match credential {
        Credential::Password(user) => {
            let perm = access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &user, name).await?;
            (user.username, perm)
        }
        Credential::Token { username, write } => {
            let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &username)
                .await?
                .0
            else {
                return Ok(Authz::Denied(Denial::Forbidden));
            };
            let mut perm =
                access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &user, name).await?;
            // `write: false` caps at read-only regardless of the owner's
            // real tier — never higher just because the token happens to
            // belong to an admin. `write: true` is capped only at the
            // owner's own real permission, never artificially raised.
            if !write {
                perm = perm.map(|p| p.min(Permission::Reporter));
            }
            (username, perm)
        }
        Credential::Pat { username, ceiling } => {
            let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &username)
                .await?
                .0
            else {
                return Ok(Authz::Denied(Denial::Forbidden));
            };
            // Same "never exceed the owner's real permission" capping as
            // `Token` above — here the cap is the token's own per-repo
            // `Permission` ceiling instead of a plain read/write bool.
            let perm = access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &user, name)
                .await?
                .map(|p| p.min(ceiling));
            (username, perm)
        }
    };
    match perm {
        Some(p) if p >= required => Ok(Authz::Allowed(Some(username))),
        _ => Ok(Authz::Denied(Denial::Forbidden)),
    }
}

enum Credential {
    Password(Box<User>),
    Token { username: String, write: bool },
    Pat { username: String, ceiling: Permission },
}

fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = BASE64.decode(encoded).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Resolves whichever credential `git clone`/`push` sent as HTTP Basic auth.
/// The username field is tried as a token secret first (GitHub's own
/// `https://<token>@host/...` convention — the password field is unused
/// for that shape, often empty or a placeholder): a local personal access
/// token (`gqpat_...`, resolved entirely within core, no federation
/// dependency) is tried first, then a federation-issued fine-grained token
/// via `POST /_internal-out/tokens/resolve` on `ctx.federation` if
/// configured; falling through to the plain username/password path covers
/// every existing local-password client unchanged. That fallback is
/// rate-limited per submitted username via [`gitquarry_core::app::LoginAttempts`]
/// — same gate `auth::web::login::handler_login_local` already applies to
/// the web login form, just unapplied here until now: this endpoint let a
/// `git clone`/`push` guess a password with no throttling at all. Checked
/// before Argon2 even runs, same "a lockout costs nothing extra" reasoning.
/// Neither token path touches `login_attempts`: a wrong/expired/out-of-scope
/// token is never a guessed password, so there's nothing to throttle.
async fn authenticate(
    ctx: &AppContext,
    headers: &HeaderMap,
    repo_uid: &crate::storage::uid::Uid,
) -> Result<Option<Credential>> {
    let Some((username, password)) = basic_credentials(headers) else {
        return Ok(None);
    };
    if let Some(cred) = resolve_local_pat(ctx, &username, repo_uid).await? {
        return Ok(Some(cred));
    }
    if let Some(federation) = &ctx.federation {
        let outcome = crate::auth::federation_token::resolve_token(federation, &username, repo_uid).await;
        if outcome.ok
            && let Some(resolved_username) = outcome.username
        {
            return Ok(Some(Credential::Token {
                username: resolved_username,
                write: outcome.write.unwrap_or(false),
            }));
        }
    }
    // Repeated failures slow this name down instead of locking it — see
    // `LoginAttempts::penalty`.
    let penalty = ctx.login_attempts.penalty(&username);
    if !penalty.is_zero() {
        tokio::time::sleep(penalty).await;
    }
    let user = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &username)
        .await?
        .0;
    // Verified against a real-but-unmatchable hash when the username is
    // unknown, so "no such user" and "wrong password" cost the same Argon2
    // work — the same equalisation `auth::web::login` does on its own form.
    let matched = password::verify_password(
        &password,
        user.as_ref()
            .and_then(|u| u.password_hash.as_deref())
            .unwrap_or(password::DUMMY_HASH.as_str()),
    );
    if let Some(user) = user.filter(|u| u.password_hash.is_some() && matched) {
        ctx.login_attempts.clear(&username);
        return Ok(Some(Credential::Password(Box::new(user))));
    }
    ctx.login_attempts.record_failure(&username);
    Ok(None)
}

/// Tries the Basic-Auth username as a local personal access token secret —
/// recognizable by its `gqpat_` prefix, so a real password or a real
/// federation token never even reaches the hash lookup below. `None` on any
/// mismatch (wrong prefix, unknown/expired secret, or `repo_uid` outside
/// the token's `scopes`) — deliberately not distinguished from "not a PAT
/// at all", so a malformed token attempt just falls through to the next
/// credential type instead of being treated as a login failure.
async fn resolve_local_pat(
    ctx: &AppContext,
    secret: &str,
    repo_uid: &crate::storage::uid::Uid,
) -> Result<Option<Credential>> {
    if !secret.starts_with("gqpat_") {
        return Ok(None);
    }
    let Some(token) = access::resolve_by_secret(ctx.backend().as_ref(), &ctx.cipher, secret).await? else {
        return Ok(None);
    };
    let Some(&ceiling) = token.scopes.get(repo_uid) else {
        return Ok(None);
    };
    let Some(user) = access::resolve_user_uid(ctx.backend().as_ref(), &ctx.cipher, &token.owner_uid).await? else {
        return Ok(None);
    };
    access::touch_pat_last_used(ctx.backend().as_ref(), &ctx.cipher, &token).await;
    Ok(Some(Credential::Pat {
        username: user.username,
        ceiling,
    }))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"gitquarry\"")],
        "credentials required",
    )
        .into_response()
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "no permission").into_response()
}

fn too_many_requests() -> Response {
    (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded, slow down").into_response()
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// One pkt-line: a 4-hex-digit length prefix (length of the whole line,
/// header included) followed by the payload — the framing git's smart-HTTP
/// advertisement needs around its `# service=...` preamble line.
fn pkt_line(data: &[u8]) -> Vec<u8> {
    let len = data.len() + 4;
    let mut out = format!("{len:04x}").into_bytes();
    out.extend_from_slice(data);
    out
}

const FLUSH_PKT: &[u8] = b"0000";

async fn info_refs_response(
    ctx: &AppContext,
    name: &str,
    repo_uid: &crate::storage::uid::Uid,
    mirror: &Mirror,
    git_subcmd: &str,
    actor: Option<&str>,
) -> Result<Response> {
    let service = format!("git-{git_subcmd}");
    let child = mirror::spawn_git_subcommand_http(
        ctx,
        git_subcmd,
        mirror::RepoRef { name, uid: repo_uid },
        mirror,
        true,
        actor,
    )?;
    let stdout = run_stateless_rpc(child, None).await?;

    let mut body = pkt_line(format!("# service={service}\n").as_bytes());
    body.extend_from_slice(FLUSH_PKT);
    body.extend_from_slice(&stdout);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, format!("application/x-{service}-advertisement")),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        body,
    )
        .into_response())
}

/// Streams the request into `git upload-pack` and the pack back out as git
/// produces it, so neither a long negotiation nor a large clone is ever
/// held whole in memory. The status line goes out before git has finished,
/// so a git failure past that point can only cut the stream short (the
/// client reports a protocol error) and is logged here.
///
/// Takes `mirror` by value because git reads it for as long as it runs:
/// the shared lock moves into the pump task and is released when git
/// exits, not when this function returns.
fn upload_pack_response(
    ctx: &AppContext,
    name: &str,
    repo_uid: &crate::storage::uid::Uid,
    mirror: Mirror,
    body: PackBody,
    actor: Option<&str>,
) -> Result<Response> {
    let mut child = mirror::spawn_git_subcommand_http(
        ctx,
        "upload-pack",
        mirror::RepoRef { name, uid: repo_uid },
        &mirror,
        false,
        actor,
    )?;
    let stdin = child.stdin.take().context("git stdin")?;
    let stdout = child.stdout.take().context("git stdout")?;
    let mut stderr = child.stderr.take().context("git stderr")?;
    tokio::spawn(async move {
        let mut err_buf = Vec::new();
        let (fed, _) = tokio::join!(feed_stdin(body, stdin), stderr.read_to_end(&mut err_buf));
        if let Err(e) = fed {
            debug!(error = %e, "http: couldn't feed upload-pack its request");
        }
        match child.wait().await {
            Ok(status) if status.success() => {}
            Ok(status) => warn!(
                status = ?status,
                stderr = %String::from_utf8_lossy(&err_buf),
                "http: git upload-pack failed"
            ),
            Err(e) => warn!(error = %e, "http: waiting for git upload-pack"),
        }
        drop(mirror);
    });
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-git-upload-pack-result")],
        Body::from_stream(stdout_chunks(stdout)),
    )
        .into_response())
}

/// `pack` is the push's request body, already spooled by [`spool_body`].
/// The response is buffered: it's only `receive-pack`'s report-status, and
/// the caller needs git's exit before it touches the mirror again anyway.
async fn receive_pack_response(
    ctx: &AppContext,
    name: &str,
    repo_uid: &crate::storage::uid::Uid,
    mirror: &Mirror,
    pack: tokio::fs::File,
    actor: Option<&str>,
) -> Result<Response> {
    let child = mirror::spawn_git_subcommand_http(
        ctx,
        "receive-pack",
        mirror::RepoRef { name, uid: repo_uid },
        mirror,
        false,
        actor,
    )?;
    let stdout = run_stateless_rpc(child, Some(pack)).await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-git-receive-pack-result")],
        stdout,
    )
        .into_response())
}

/// A pack request body with its `Content-Encoding` undone as it streams in:
/// git's HTTP client gzips any upload-pack request over 1 KiB (a clone of a
/// repo with many refs, a fetch with many haves), while `git upload-pack`
/// itself only reads the raw protocol.
struct PackBody {
    chunks: axum::body::BodyDataStream,
    gzip: Option<flate2::write::GzDecoder<Vec<u8>>>,
}

impl PackBody {
    fn from_request(req: Request) -> Result<Self, ClientError> {
        let gzip = match req.headers().get(header::CONTENT_ENCODING).map(|v| v.as_bytes()) {
            None | Some(b"identity") => None,
            Some(b"gzip" | b"x-gzip") => Some(flate2::write::GzDecoder::new(Vec::new())),
            Some(_) => return Err(ClientError::BadRequest("unsupported Content-Encoding")),
        };
        Ok(PackBody {
            chunks: req.into_body().into_data_stream(),
            gzip,
        })
    }

    /// The next run of decoded bytes; `None` once the body and the decoder
    /// are both drained. Callers stop at the first `Err`.
    async fn next(&mut self) -> Option<Result<Bytes>> {
        const MALFORMED: ClientError = ClientError::BadRequest("malformed gzip request body");
        loop {
            let Some(chunk) = self.chunks.next().await else {
                let tail = self.gzip.take()?.finish();
                return Some(tail.map(Bytes::from).map_err(|_| MALFORMED.into()));
            };
            let Ok(chunk) = chunk else {
                return Some(Err(ClientError::BadRequest("request body ended early").into()));
            };
            let Some(gzip) = &mut self.gzip else {
                return Some(Ok(chunk));
            };
            if std::io::Write::write_all(gzip, &chunk).is_err() {
                return Some(Err(MALFORMED.into()));
            }
            let decoded = std::mem::take(gzip.get_mut());
            if !decoded.is_empty() {
                return Some(Ok(Bytes::from(decoded)));
            }
        }
    }
}

/// Copies the request body into git's stdin chunk by chunk, closing stdin
/// at the end so git sees EOF.
async fn feed_stdin(mut body: PackBody, mut stdin: ChildStdin) -> Result<()> {
    while let Some(chunk) = body.next().await {
        stdin.write_all(&chunk?).await.context("writing git stdin")?;
    }
    Ok(())
}

const STDOUT_CHUNK_BYTES: usize = 64 * 1024;

/// git's stdout as a response body stream. Ends at the first read error
/// rather than retrying a pipe that already failed. Fused, because a body
/// adapter in front of this layer (the web UI's compression) may poll once
/// more after the end, and a bare `unfold` panics on that.
fn stdout_chunks(stdout: ChildStdout) -> impl futures::Stream<Item = std::io::Result<Bytes>> {
    futures::stream::unfold(Some(stdout), |state| async move {
        let mut stdout = state?;
        let mut buf = vec![0; STDOUT_CHUNK_BYTES];
        match stdout.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), Some(stdout)))
            }
            Err(e) => Some((Err(e), None)),
        }
    })
    .fuse()
}

/// Writes a push's request body to an already-unlinked temp file under
/// `mirrors_root` — on disk next to the mirrors, rather than a `/tmp` that
/// may be a tmpfs and put the pack straight back in RAM — and rewinds it
/// for reading. Capped at [`MAX_PACK_BODY_BYTES`] (413 past it) so a
/// runaway client can't fill the disk either.
async fn spool_body(ctx: &AppContext, mut body: PackBody) -> Result<tokio::fs::File> {
    tokio::fs::create_dir_all(&ctx.mirrors_root)
        .await
        .with_context(|| format!("creating {}", ctx.mirrors_root.display()))?;
    let dir = ctx.mirrors_root.clone();
    let file = tokio::task::spawn_blocking(move || tempfile::tempfile_in(dir))
        .await
        .context("joining the spool file creation")?
        .context("creating the push spool file")?;
    let mut file = tokio::fs::File::from_std(file);
    let mut written = 0;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        written += chunk.len();
        if written > MAX_PACK_BODY_BYTES {
            return Err(ClientError::TooLarge.into());
        }
        file.write_all(&chunk).await.context("writing the push spool file")?;
    }
    file.flush().await.context("flushing the push spool file")?;
    file.rewind().await.context("rewinding the push spool file")?;
    Ok(file)
}

/// Writes `request_body` (if any) to `child`'s stdin, closes it, and reads
/// stdout/stderr to completion — all concurrently, so a large pack in
/// either direction can't deadlock on a full OS pipe buffer while nobody's
/// draining the other end (same reasoning as `ssh.rs`'s `pump_data`/
/// `pump_extended_data` `tokio::join!`, just request/response instead of a
/// persistent channel).
async fn run_stateless_rpc(mut child: Child, request_body: Option<tokio::fs::File>) -> Result<Vec<u8>> {
    let mut stdin = child.stdin.take().context("git stdin")?;
    let mut stdout = child.stdout.take().context("git stdout")?;
    let mut stderr = child.stderr.take().context("git stderr")?;

    let write_fut = async {
        if let Some(mut body) = request_body {
            // Ignored, because git stopping early is reported by its exit
            // status below, with its own stderr.
            let _ = tokio::io::copy(&mut body, &mut stdin).await;
        }
        drop(stdin);
    };
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let (_, out_res, err_res) = tokio::join!(
        write_fut,
        stdout.read_to_end(&mut out_buf),
        stderr.read_to_end(&mut err_buf),
    );
    out_res.context("reading git stdout")?;
    err_res.context("reading git stderr")?;

    let status = child.wait().await.context("waiting for git --stateless-rpc")?;
    if !status.success() {
        bail!(
            "git --stateless-rpc exited with error: {status:?}: {}",
            String::from_utf8_lossy(&err_buf)
        );
    }
    Ok(out_buf)
}
