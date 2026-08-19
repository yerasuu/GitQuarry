//! Embedded SSH server: one of the two publicly supported transports
//! (alongside [`super::http`], git smart-HTTP) for cloning/pushing via
//! `ssh://`. Requires pubkey auth and enforces per-repo permissions
//! (read/write) stored in the bucket — see [`crate::auth::access`].
//!
//! Doesn't rely on the system's sshd or OS users: everything (host key and
//! authorized keys) lives in the bucket, so identity isn't lost on restart
//! in a fresh container (the server is meant to be dockerized).

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{Permission, User};
use crate::crypto::Cipher;
use crate::domain;
use crate::mirror::{spawn_git_subcommand, sync_bare_repo_from_s3};
use crate::storage::entities::Visibility;
use crate::storage::{Backend, Cas};
use crate::validate;
use anyhow::{Context, Result, bail};
use russh::keys::{Algorithm, PrivateKey, PublicKey, ssh_key::LineEnding};
use russh::server::{self, Auth, Config, Handle, Msg, Server as _, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use s3db::Singleton;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::ChildStdin;

const HOST_KEY: Singleton = Singleton::new(".conf/ssh_host_key.json");
/// Size of the read buffers when pumping git's stdout/stderr to the SSH
/// channel.
const STDOUT_CHUNK: usize = 32 * 1024;
const STDERR_CHUNK: usize = 4096;

pub async fn run_server(bind_addr: &str, ctx: Arc<AppContext>) -> Result<()> {
    let host_key = load_or_create_host_key(ctx.backend.as_ref(), &ctx.cipher).await?;
    let config = Arc::new(Config {
        keys: vec![host_key],
        // Pubkey only: no password or keyboard-interactive, so the client
        // doesn't even offer them.
        methods: MethodSet::from(&[MethodKind::PublicKey][..]),
        ..Default::default()
    });

    let socket = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("couldn't listen on {bind_addr}"))?;
    eprintln!(
        "gitquarry: ssh:// listening on {bind_addr} -> bucket '{}' (mirrors in {})",
        ctx.bucket,
        ctx.mirrors_root.display()
    );

    let mut server = SshServer { ctx };
    server.run_on_socket(config, &socket).await.context("SSH server failed")
}

/// Loads the host key persisted in the bucket, or generates it and saves it
/// with CAS (create-if-absent) if it's the first time. Lives in the bucket
/// (not on local disk) so identity isn't lost across restarts in ephemeral
/// containers. It's a PRIVATE key: `HOST_KEY` encrypts it like any other
/// bucket data, it never stays in the clear.
async fn load_or_create_host_key(backend: &dyn Backend, cipher: &Cipher) -> Result<PrivateKey> {
    #[derive(Debug, Default, Serialize, Deserialize)]
    struct HostKeyDoc {
        #[serde(default)]
        openssh: Option<String>,
    }

    let (doc, _etag) = HOST_KEY.get::<HostKeyDoc>(backend, cipher).await?;
    if let Some(openssh) = doc.openssh {
        return PrivateKey::from_openssh(&openssh).context("invalid host key in bucket");
    }

    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).context("generating SSH host key")?;
    let openssh = key
        .to_openssh(LineEnding::LF)
        .context("serializing SSH host key")?
        .to_string();
    let new_doc = HostKeyDoc { openssh: Some(openssh) };
    match HOST_KEY.put(backend, cipher, &new_doc, None).await? {
        Cas::Ok => Ok(key),
        // Two instances starting at once: doesn't matter which wins, we use
        // whichever ended up stored.
        Cas::Conflict => {
            let (doc, _etag) = HOST_KEY.get::<HostKeyDoc>(backend, cipher).await?;
            let openssh = doc
                .openssh
                .context("host key disappeared from bucket after a CAS conflict")?;
            PrivateKey::from_openssh(&openssh).context("invalid host key in bucket")
        }
    }
}

/// `Some(io_err)` if `error` is `russh::Error::IO` wrapping one of the
/// handful of `std::io::ErrorKind`s a client hanging up mid-connection
/// actually produces — split out from [`SshServer::handle_session_error`]
/// so the classification itself stays unit-testable without a live
/// `russh` session.
pub fn benign_disconnect_reason(error: &anyhow::Error) -> Option<&std::io::Error> {
    use std::io::ErrorKind;
    let russh::Error::IO(io_err) = error.downcast_ref::<russh::Error>()? else {
        return None;
    };
    matches!(
        io_err.kind(),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
    )
    .then_some(io_err)
}

#[derive(Clone)]
struct SshServer {
    ctx: Arc<AppContext>,
}

impl server::Server for SshServer {
    type Handler = ConnHandler;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> ConnHandler {
        ConnHandler {
            ctx: Arc::clone(&self.ctx),
            cache: s3db::RequestCache::new(Arc::clone(&self.ctx.backend)),
            authorized_user: None,
            stdin: None,
            active_channel: None,
        }
    }

    /// `russh`'s own internal packet-read loop (not any of our `Handler`
    /// methods) reports here whenever the underlying TCP connection dies —
    /// most commonly `russh::Error::IO` wrapping a raw `std::io::Error`
    /// whose `Display` is tokio's own generic `"early eof"`/"broken pipe"/
    /// "connection reset" wording (`#[error(transparent)]` on that variant
    /// forwards it verbatim). That's not a gitquarry bug, just a client
    /// that hung up mid-connection (network drop, `git` process killed,
    /// aggressive client timeout) — logged as a clear, low-noise line
    /// instead of the cryptic raw transport message. Anything else (a
    /// genuine protocol/auth failure) still gets the full `{error:#}`.
    fn handle_session_error(&mut self, error: <Self::Handler as server::Handler>::Error) {
        match benign_disconnect_reason(&error) {
            Some(io_err) => eprintln!("gitquarry: SSH client disconnected before finishing (io error: {io_err})"),
            None => eprintln!("gitquarry: SSH session failed: {error:#}"),
        }
    }
}

/// One `ConnHandler` per SSH connection. A git client opens exactly one
/// session channel per operation (clone/fetch/push), so it's enough to
/// store the current process's stdin instead of a per-channel map.
struct ConnHandler {
    ctx: Arc<AppContext>,
    /// Memoizes reads for this one connection's lifetime only — never
    /// process-wide, since gitquarry runs multiple instances against the
    /// same bucket and a stale cached `User`/permission record could let a
    /// revoked permission or key stay "live" longer than it should. Shared
    /// by `auth_publickey` and `start_git_service`, which otherwise each
    /// independently re-fetch the same user's record.
    cache: s3db::RequestCache,
    /// Set after a successful `auth_publickey`: the `.conf/access.json`
    /// user whose pubkeys include the one the client offered — regardless
    /// of which of their pubkeys it was.
    authorized_user: Option<User>,
    /// Stdin of the running `git upload-pack`/`git receive-pack`, so
    /// `data()` (client input) can write to it.
    stdin: Option<ChildStdin>,
    active_channel: Option<ChannelId>,
}

impl server::Handler for ConnHandler {
    type Error = anyhow::Error;

    async fn auth_publickey(&mut self, _user: &str, public_key: &PublicKey) -> Result<Auth, Self::Error> {
        // The username from the url ssh://<user>@host is ignored: identity
        // is the pubkey (resolved to a User), not this string (same as
        // github/gitolite).
        let fingerprint = access::fingerprint_of(public_key);
        let user = access::resolve_by_fingerprint(&self.cache, &self.ctx.cipher, &fingerprint).await?;
        match user {
            Some(user) => {
                self.authorized_user = Some(user);
                Ok(Auth::Accept)
            }
            None => Ok(Auth::reject()),
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Err(e) = self.start_git_service(channel, data, session).await {
            eprintln!("gitquarry: ssh: request rejected: {e:#}");
            let _ = session
                .handle()
                .extended_data(channel, 1, format!("gitquarry: {e:#}\n").into_bytes())
                .await;
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _session: &mut Session) -> Result<(), Self::Error> {
        if self.active_channel == Some(channel)
            && let Some(stdin) = self.stdin.as_mut()
        {
            let _ = stdin.write_all(data).await;
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        // The client finished sending (pack data of a push, typically).
        // Dropping stdin closes the fd, which is how git gets told no more
        // input is coming.
        if self.active_channel == Some(channel) {
            self.stdin = None;
        }
        Ok(())
    }
}

impl ConnHandler {
    async fn start_git_service(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<()> {
        let (git_subcmd, raw_path) = parse_exec_command(data)?;
        let name = validate::sanitize_path(&raw_path)?;

        let username = self
            .authorized_user
            .as_ref()
            .context("not authenticated")?
            .username
            .clone();
        // Re-resolved fresh rather than reused from `auth_publickey` time:
        // a permission/group change shouldn't wait for the next connection
        // to take effect. `effective_permission_for` only fetches the
        // acting user's own record plus (if the repo's namespace is a
        // group) that one group's record — not the whole bucket's worth of
        // users/groups the old `effective_permission` needed.
        let permission = access::effective_permission_for(&self.cache, &self.ctx.cipher, &username, &name)
            .await?
            .with_context(|| format!("no access to '{name}'"))?;
        let required = required_permission(git_subcmd);
        if permission < required {
            let verb = if required == Permission::Developer {
                "write"
            } else {
                "read"
            };
            bail!("key has no {verb} permission on '{name}'");
        }

        let resolved = match self.ctx.resolve_repo(&name).await? {
            Some(r) => r,
            // `receive-pack` here already implies at least `Permission::Developer` (the
            // bail above), so a nonexistent repo under a namespace this
            // pusher can write to is exactly "first push creates it" —
            // same implicit-create `webui::repo_new` assumes for a personal
            // namespace, just reached via `git push` instead of `/_new`.
            // Gated by `allow_push_create` (off by default, see
            // `config::RawConfig::allow_push_create`'s doc comment): a
            // typo'd repo name shouldn't silently create a new empty repo.
            // `upload-pack` never takes this branch: cloning something that
            // doesn't exist yet is an error, not an implicit creation.
            //
            // Goes through `domain::repo::create` (writes a real
            // `RepoConfig`, not just the identity claim `AppContext::create_repo`
            // alone would) — the fix for a push-created repo previously
            // showing up as a blank row on `/`/`/_profile/<user>`, since
            // those list by `RepoConfig::full_name`, which nothing used to
            // write for this path.
            None if git_subcmd == "receive-pack" && self.ctx.allow_push_create => {
                let owner = match name.split_once('/') {
                    Some((namespace, _)) => domain::repo::resolve_owner_ref(&self.ctx, namespace).await?,
                    None => None,
                };
                domain::repo::create(&self.ctx, &name, owner, None, Visibility::default())
                    .await?
                    .with_context(|| format!("couldn't create '{name}'"))?
            }
            None => bail!("repo '{name}' doesn't exist"),
        };
        // Same reasoning as `git::http`'s own gate: a soft-deleted repo
        // (`domain::repo::delete`) is invisible over every transport, not
        // just blocked on push like an archived one.
        let (config, _etag) = resolved.remote.get_repo_config().await?;
        if config.deleted_at.is_some() {
            bail!("repo '{name}' doesn't exist");
        }

        // Snapshot taken before the push so the post-push block can diff
        // against it — the ref-update data the `pre-receive` hook parses
        // never reaches this process, it runs as a separate subprocess (see
        // `events::diff_refs`'s own doc comment).
        let before_refs = if git_subcmd == "receive-pack" {
            resolved.remote.get_manifest().await.ok().map(|(m, _)| m.refs)
        } else {
            None
        };

        let mirror = self.ctx.open_mirror(&resolved).await?;
        let mut child = spawn_git_subcommand(
            &self.ctx,
            git_subcmd,
            crate::mirror::RepoRef {
                name: &name,
                uid: &resolved.uid,
            },
            &mirror,
            None,
            Stdio::piped(),
            Some(&username),
        )?;
        let stdin = child.stdin.take().context("git stdin")?;
        let stdout = child.stdout.take().context("git stdout")?;
        let stderr = child.stderr.take().context("git stderr")?;

        self.stdin = Some(stdin);
        self.active_channel = Some(channel);
        session.channel_success(channel)?;

        let handle = session.handle();
        let git_subcmd = git_subcmd.to_string();
        let language_ctx = Arc::clone(&self.ctx);
        let language_uid = resolved.uid.clone();
        let event_ctx = Arc::clone(&self.ctx);
        let event_uid = resolved.uid.clone();
        let event_remote = resolved.remote.clone();
        let pusher = username.clone();
        tokio::spawn(async move {
            let (_, _, status) = tokio::join!(
                pump_data(&handle, channel, stdout),
                pump_extended_data(&handle, channel, stderr),
                child.wait(),
            );

            if git_subcmd == "receive-pack" && matches!(&status, Ok(s) if s.success()) {
                // the pre-receive hook already synced S3; refresh the
                // mirror just in case (another concurrent push, deleted
                // refs, etc), same as git smart-HTTP does after a push.
                if let Err(e) = sync_bare_repo_from_s3(&mirror.remote, &mirror.path).await {
                    eprintln!("gitquarry: ssh: couldn't re-sync after push: {e:#}");
                }
                // Detached: see `domain::repo::update_primary_language`'s
                // own doc comment for why it takes a fresh lock instead of
                // reusing `mirror`'s (about to be dropped at the end of
                // this scope). `update_search_index` shares the same
                // reasoning and the same detached task — both are
                // best-effort bookkeeping the client's own push response
                // never waits on.
                let path = mirror.path.clone();
                tokio::spawn(async move {
                    domain::repo::update_primary_language(&language_ctx, &language_uid, &path).await;
                    domain::repo::update_search_index(&language_ctx, &language_uid, &path).await;
                });
                // Detached, same reasoning as the language/search-index
                // updates above: webhook/CI delivery must never make the
                // pusher wait.
                if let Some(before_refs) = before_refs {
                    tokio::spawn(async move {
                        if let Ok((after, _etag)) = event_remote.get_manifest().await {
                            let refs = crate::events::diff_refs(&before_refs, &after.refs);
                            if !refs.is_empty() {
                                crate::events::emit(
                                    &event_ctx,
                                    crate::events::RepoEvent::Push {
                                        repo_uid: event_uid,
                                        refs,
                                        pusher: Some(pusher),
                                    },
                                );
                            }
                        }
                    });
                }
            }

            let code = status.ok().and_then(|s| s.code()).unwrap_or(1) as u32;
            let _ = handle.exit_status_request(channel, code).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });

        Ok(())
    }
}

async fn pump_data(handle: &Handle, channel: ChannelId, mut r: impl AsyncRead + Unpin) {
    let mut buf = [0u8; STDOUT_CHUNK];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if handle.data(channel, buf[..n].to_vec()).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn pump_extended_data(handle: &Handle, channel: ChannelId, mut r: impl AsyncRead + Unpin) {
    let mut buf = [0u8; STDERR_CHUNK];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // ext == 1: SSH_EXTENDED_DATA_STDERR.
                if handle.extended_data(channel, 1, buf[..n].to_vec()).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Minimum tier `git_subcmd` needs to proceed — `Developer` to push,
/// `Reporter` to clone/fetch, same floor `git::http::authorize` already
/// applies for the equivalent smart-HTTP paths. Before this existed,
/// `upload-pack` had no tier check at all beyond "`effective_permission_for`
/// returned *some* permission" — a `Guest` (explicitly "grants no code-read
/// rights at all" per [`Permission`]'s own doc comment) could still clone
/// over SSH, even though HTTP/web both require at least `Reporter` for the
/// same repo.
pub fn required_permission(git_subcmd: &str) -> Permission {
    if git_subcmd == "receive-pack" {
        Permission::Developer
    } else {
        Permission::Reporter
    }
}

/// Parses the command a git client sends via SSH `exec`:
/// `git-upload-pack '/repo/name'` or `git-receive-pack '/repo/name'`. The
/// path usually comes in POSIX sh-style single quotes (mirroring the
/// `shell_quote` `mirror/mod.rs` already uses for the hook); if it doesn't
/// come quoted it's used as-is (`validate::sanitize_path` validates the result).
pub fn parse_exec_command(raw: &[u8]) -> Result<(&'static str, String)> {
    let s = std::str::from_utf8(raw).context("ssh command isn't utf8")?.trim();
    let (cmd, rest) = s
        .split_once(' ')
        .with_context(|| format!("ssh command with no argument: {s}"))?;
    let git_subcmd = match cmd {
        "git-upload-pack" => "upload-pack",
        "git-receive-pack" => "receive-pack",
        other => bail!("unsupported ssh command: {other}"),
    };
    let rest = rest.trim();
    let path = match rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        Some(inner) => inner.replace("'\\''", "'"),
        None => rest.to_string(),
    };
    Ok((git_subcmd, path))
}
