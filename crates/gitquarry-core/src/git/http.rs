//! git smart-HTTP: `GET .../info/refs?service=git-upload-pack|git-receive-pack`,
//! `POST .../git-upload-pack`, `POST .../git-receive-pack` — the protocol a
//! plain `git clone`/`push http://...` speaks, see
//! <https://git-scm.com/docs/http-protocol>. One of the two publicly
//! supported transports (alongside [`super::ssh`]); this file owns the wire
//! protocol only — it never renders HTML and never calls into
//! [`crate::webui::repo`]/[`crate::webui::repo_settings`], and vice versa. It
//! has to live alongside [`crate::webui`]'s router rather than owning its own
//! listener (unlike [`super::ssh`]) because both transports are meant to
//! share one HTTP port — so it's wired in as [`middleware`], a layer over
//! the whole router (see [`crate::webui::run_server`]): it short-circuits the
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
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gitquarry_auth_crypto::password;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;

/// Request bodies for `POST .../info/lfs/objects/batch` only ever list
/// oids/sizes, never file content — a generous cap, well above any real
/// batch, just to keep a malformed/hostile request from buffering unbounded
/// memory before it's even parsed.
const MAX_LFS_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// `upload-pack`/`receive-pack` request bodies are real pack data, so this
/// stays generous enough to never trip on a legitimate push/fetch (well past
/// any real monorepo pack) -- it exists only so a runaway or hostile client
/// can't make the server buffer an unbounded body into memory before `git`
/// ever sees a byte of it.
const MAX_PACK_BODY_BYTES: usize = 16 * 1024 * 1024 * 1024;

#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    InfoRefs,
    UploadPack,
    ReceivePack,
    LfsBatch,
    LfsObject,
    LfsLocks,
    LfsLocksVerify,
    LfsLockUnlock,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    #[cfg(any(test, feature = "test-util"))]
    pub fn valid_method(self, method: &Method) -> bool {
        Self::valid_method_impl(self, method)
    }
    #[cfg(not(any(test, feature = "test-util")))]
    fn valid_method(self, method: &Method) -> bool {
        Self::valid_method_impl(self, method)
    }
    fn valid_method_impl(self, method: &Method) -> bool {
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
#[cfg(any(test, feature = "test-util"))]
pub fn classify(path: &str) -> Option<(&str, Kind, Option<&str>)> {
    classify_impl(path)
}
#[cfg(not(any(test, feature = "test-util")))]
fn classify(path: &str) -> Option<(&str, Kind, Option<&str>)> {
    classify_impl(path)
}

fn classify_impl(path: &str) -> Option<(&str, Kind, Option<&str>)> {
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
/// ([`AppContext::trust_forwarded_for`]) — the first `X-Forwarded-For` hop.
///
/// Behind a reverse proxy the socket address is the proxy's, so every
/// anonymous client in the world shares one bucket and one of them can
/// spend it for everybody. Trusting the header without a proxy that
/// overwrites it is the opposite failure: each request picks its own
/// bucket, and the limit stops existing. Neither is safe to guess, hence
/// the setting.
#[cfg(any(test, feature = "test-util"))]
pub fn client_key(ctx: &AppContext, headers: &HeaderMap, socket_ip: std::net::IpAddr) -> String {
    client_key_impl(ctx, headers, socket_ip)
}
#[cfg(not(any(test, feature = "test-util")))]
fn client_key(ctx: &AppContext, headers: &HeaderMap, socket_ip: std::net::IpAddr) -> String {
    client_key_impl(ctx, headers, socket_ip)
}

fn client_key_impl(ctx: &AppContext, headers: &HeaderMap, socket_ip: std::net::IpAddr) -> String {
    if !ctx.trust_forwarded_for {
        return socket_ip.to_string();
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        // `client, proxy1, proxy2` — the first entry is the hop that faced
        // the client, same reading `webui::cookies::is_https` gives
        // `X-Forwarded-Proto`.
        .and_then(|v| v.split(',').next())
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
    let repo = repo.to_string();
    let oid = oid.map(str::to_string);
    let client = client_key(&ctx, req.headers(), addr.ip());
    match handle(&ctx, &repo, kind, oid.as_deref(), client, req).await {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("gitquarry: git-http: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn handle(
    ctx: &Arc<AppContext>,
    raw_repo: &str,
    kind: Kind,
    oid: Option<&str>,
    client_key: String,
    req: Request,
) -> Result<Response> {
    let name = validate::sanitize_path(raw_repo)?;
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    // A soft-deleted repo (`domain::repo::delete`) is invisible over every
    // transport until restored or purged, not just blocked on push like an
    // archived one — same treatment as a repo that never existed.
    if config.deleted_at.is_some() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    match kind {
        Kind::InfoRefs | Kind::UploadPack | Kind::ReceivePack => {
            let required = if kind == Kind::ReceivePack {
                Permission::Developer
            } else {
                Permission::Reporter
            };
            let actor = match authorize(ctx, req.headers(), &name, &resolved.uid, config.visibility, required).await? {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(reason.into_git_response()),
            };
            // Clone/fetch flood + scraping guard (`GitRateLimiter`'s own doc
            // comment) — `ReceivePack` (push) is deliberately not covered,
            // same "read-heavy scraping is the actual threat model here"
            // scoping that doc comment describes.
            if kind != Kind::ReceivePack {
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
            if kind != Kind::ReceivePack {
                crate::mirror::sync_external_before_serving(ctx, &resolved).await;
            }
            let mirror = ctx.open_mirror(&resolved).await?;
            match kind {
                Kind::InfoRefs => {
                    let query = req.uri().query().unwrap_or("");
                    let service = query_param(query, "service").context("missing ?service=")?;
                    info_refs_response(ctx, &name, &resolved.uid, &mirror, service, actor.as_deref()).await
                }
                Kind::UploadPack => {
                    pack_response(ctx, "upload-pack", &name, &resolved.uid, &mirror, req, actor.as_deref()).await
                }
                Kind::ReceivePack => {
                    // Snapshot before the push so the post-push block can
                    // diff against it — see `events::diff_refs`'s own doc
                    // comment for why this process has no other way to
                    // learn which refs a push touched.
                    let before_refs = resolved.remote.get_manifest().await.ok().map(|(m, _)| m.refs);
                    let resp = pack_response(
                        ctx,
                        "receive-pack",
                        &name,
                        &resolved.uid,
                        &mirror,
                        req,
                        actor.as_deref(),
                    )
                    .await?;
                    // The pre-receive hook already pushed to the bucket; this just
                    // refreshes the local mirror for the next reader, same as
                    // `ssh.rs` does after a push.
                    if let Err(e) = mirror::sync_bare_repo_from_s3(&mirror.remote, &mirror.path).await {
                        eprintln!("gitquarry: http: couldn't re-sync after push: {e:#}");
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
                    if let Some(before_refs) = before_refs {
                        let event_ctx = Arc::clone(ctx);
                        let event_uid = resolved.uid.clone();
                        let event_remote = resolved.remote.clone();
                        let pusher = actor.clone();
                        tokio::spawn(async move {
                            if let Ok((after, _etag)) = event_remote.get_manifest().await {
                                let refs = crate::events::diff_refs(&before_refs, &after.refs);
                                if !refs.is_empty() {
                                    crate::events::emit(
                                        &event_ctx,
                                        crate::events::RepoEvent::Push {
                                            repo_uid: event_uid,
                                            refs,
                                            pusher,
                                        },
                                    );
                                }
                            }
                        });
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
            let body = axum::body::to_bytes(req.into_body(), MAX_LFS_BATCH_BYTES)
                .await
                .context("reading LFS batch body")?;
            let required = crate::git::lfs::required_permission(&crate::git::lfs::peek_operation(&body));
            let actor = match authorize(ctx, &headers, &name, &resolved.uid, config.visibility, required).await? {
                Authz::Allowed(actor) => actor,
                Authz::Denied(reason) => return Ok(crate::git::lfs::denial_response(reason)),
            };
            let _ = actor; // not attributed anywhere for LFS (no git hook runs for this path)
            crate::git::lfs::batch_response(&resolved.remote, &name, &headers, body).await
        }
        Kind::LfsObject => {
            let oid = oid.context("missing LFS oid")?;
            validate::validate_lfs_oid(oid)?;
            let is_upload = req.method() == Method::PUT;
            let required = if is_upload {
                Permission::Developer
            } else {
                Permission::Reporter
            };
            let actor = match authorize(ctx, req.headers(), &name, &resolved.uid, config.visibility, required).await? {
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
            let actor = match authorize(ctx, &headers, &name, &resolved.uid, config.visibility, required).await? {
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
                let body = axum::body::to_bytes(req.into_body(), MAX_LFS_BATCH_BYTES)
                    .await
                    .context("reading LFS lock create body")?;
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
                &name,
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
                &name,
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
            let body = axum::body::to_bytes(req.into_body(), MAX_LFS_BATCH_BYTES)
                .await
                .context("reading LFS unlock body")?;
            crate::git::lfs::unlock_response(&backend, &ctx.cipher, &resolved.uid, id, &requester, body).await
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
#[derive(Debug)]
pub enum Authz {
    Allowed(Option<String>),
    Denied(Denial),
}
#[cfg(not(any(test, feature = "test-util")))]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "test-util"))]
pub enum Denial {
    Unauthorized,
    Forbidden,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(not(any(test, feature = "test-util")))]
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
/// web login form uses) or a personal access token naming this exact repo —
/// and at least `required` either way.
#[cfg(any(test, feature = "test-util"))]
pub async fn authorize(
    ctx: &AppContext,
    headers: &HeaderMap,
    name: &str,
    repo_uid: &crate::storage::uid::Uid,
    visibility: Visibility,
    required: Permission,
) -> Result<Authz> {
    authorize_impl(ctx, headers, name, repo_uid, visibility, required).await
}
#[cfg(not(any(test, feature = "test-util")))]
async fn authorize(
    ctx: &AppContext,
    headers: &HeaderMap,
    name: &str,
    repo_uid: &crate::storage::uid::Uid,
    visibility: Visibility,
    required: Permission,
) -> Result<Authz> {
    authorize_impl(ctx, headers, name, repo_uid, visibility, required).await
}

async fn authorize_impl(
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
    Pat { username: String, ceiling: Permission },
}

#[cfg(any(test, feature = "test-util"))]
pub fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    basic_credentials_impl(headers)
}
#[cfg(not(any(test, feature = "test-util")))]
fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    basic_credentials_impl(headers)
}

fn basic_credentials_impl(headers: &HeaderMap) -> Option<(String, String)> {
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
/// token (`gqpat_...`) is tried first; falling through to the plain
/// username/password path covers every existing local-password client
/// unchanged. That fallback is
/// rate-limited per submitted username via [`gitquarry_core::app::LoginAttempts`]
/// — same gate `auth::web::login::handler_login_local` already applies to
/// the web login form, just unapplied here until now: this endpoint let a
/// `git clone`/`push` guess a password with no throttling at all. Checked
/// before Argon2 even runs, same "a lockout costs nothing extra" reasoning.
/// The token path never touches `login_attempts`: a wrong/expired/out-of-
/// scope token is never a guessed password, so there's nothing to throttle.
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
/// recognizable by its `gqpat_` prefix, so a real password never even
/// reaches the hash lookup below. `None` on any
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

#[cfg(any(test, feature = "test-util"))]
pub fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query_param_impl(query, key)
}
#[cfg(not(any(test, feature = "test-util")))]
fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query_param_impl(query, key)
}

fn query_param_impl<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// One pkt-line: a 4-hex-digit length prefix (length of the whole line,
/// header included) followed by the payload — the framing git's smart-HTTP
/// advertisement needs around its `# service=...` preamble line.
#[cfg(any(test, feature = "test-util"))]
pub fn pkt_line(data: &[u8]) -> Vec<u8> {
    pkt_line_impl(data)
}
#[cfg(not(any(test, feature = "test-util")))]
fn pkt_line(data: &[u8]) -> Vec<u8> {
    pkt_line_impl(data)
}

fn pkt_line_impl(data: &[u8]) -> Vec<u8> {
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
    service: &str,
    actor: Option<&str>,
) -> Result<Response> {
    let git_subcmd = match service {
        "git-upload-pack" => "upload-pack",
        "git-receive-pack" => "receive-pack",
        other => bail!("unsupported service: {other}"),
    };
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

async fn pack_response(
    ctx: &AppContext,
    git_subcmd: &str,
    name: &str,
    repo_uid: &crate::storage::uid::Uid,
    mirror: &Mirror,
    req: Request,
    actor: Option<&str>,
) -> Result<Response> {
    let body = axum::body::to_bytes(req.into_body(), MAX_PACK_BODY_BYTES)
        .await
        .context("reading request body")?;
    let child = mirror::spawn_git_subcommand_http(
        ctx,
        git_subcmd,
        mirror::RepoRef { name, uid: repo_uid },
        mirror,
        false,
        actor,
    )?;
    let stdout = run_stateless_rpc(child, Some(body)).await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, format!("application/x-git-{git_subcmd}-result"))],
        stdout,
    )
        .into_response())
}

/// Writes `request_body` (if any) to `child`'s stdin, closes it, and reads
/// stdout/stderr to completion — all concurrently, so a large pack in
/// either direction can't deadlock on a full OS pipe buffer while nobody's
/// draining the other end (same reasoning as `ssh.rs`'s `pump_data`/
/// `pump_extended_data` `tokio::join!`, just request/response instead of a
/// persistent channel).
async fn run_stateless_rpc(mut child: Child, request_body: Option<Bytes>) -> Result<Vec<u8>> {
    let mut stdin = child.stdin.take().context("git stdin")?;
    let mut stdout = child.stdout.take().context("git stdout")?;
    let mut stderr = child.stderr.take().context("git stderr")?;

    let write_fut = async {
        if let Some(body) = request_body {
            let _ = stdin.write_all(&body).await;
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
