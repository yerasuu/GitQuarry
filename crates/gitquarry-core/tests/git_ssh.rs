//! Drives `git::ssh::run_server` over a real socket with the system `ssh`
//! client and `git`, so every assertion goes through pubkey auth, exec
//! parsing and the permission gate exactly as a remote user hits them.
//! Skipped (not failed) if `ssh`/`ssh-keygen` isn't on `PATH`, same
//! tolerance `gitquarry-gpg`'s tests have for `gpg`.

use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user::{self, NewUser};
use gitquarry_core::auth::entities::Permission;
use gitquarry_i18n::Locale;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const REPO: &str = "team/proj";

fn tools_available() -> bool {
    ["ssh", "ssh-keygen"]
        .iter()
        .all(|tool| std::process::Command::new(tool).arg("-V").output().is_ok())
}

struct Server {
    ctx: Arc<AppContext>,
    port: u16,
    // Held so the bucket and generated keys outlive every client call.
    _bucket: tempfile::TempDir,
    keys: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        let bucket = tempfile::tempdir().unwrap();
        let ctx = Arc::new(gitquarry_test_support::ctx(bucket.path()));
        ctx.create_repo(REPO).await.unwrap().unwrap();

        // run_server only takes an address, so a free port is found by
        // binding and releasing it first.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let server_ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            gitquarry_core::git::ssh::run_server(&format!("127.0.0.1:{port}"), server_ctx)
                .await
                .unwrap();
        });
        // The listener only binds after the host key is loaded/created in
        // the bucket, so wait until it accepts.
        for _ in 0..100 {
            if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Server {
            ctx,
            port,
            _bucket: bucket,
            keys: tempfile::tempdir().unwrap(),
        }
    }

    /// Creates `username` with a fresh ed25519 key and, if `perm` is set,
    /// that grant on [`REPO`]; returns the private key's path.
    async fn add_user(&self, username: &str, perm: Option<Permission>) -> PathBuf {
        let key = self.keys.path().join(username);
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", username, "-f"])
            .arg(&key)
            .status()
            .unwrap();
        assert!(status.success());
        let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();

        user::create(
            &self.ctx,
            NewUser {
                username,
                email: None,
                pubkey: None,
                password_plain: None,
                is_admin: false,
                repos_field: "",
                locale: Locale::En,
            },
        )
        .await
        .unwrap()
        .unwrap();
        user::add_pubkey(&self.ctx, username, pubkey.trim(), Locale::En)
            .await
            .unwrap();
        if perm.is_some() {
            user::set_repo_perm(&self.ctx, username, REPO, perm, Locale::En)
                .await
                .unwrap();
        }
        key
    }

    fn ssh_args(&self, key: &Path) -> Vec<String> {
        vec![
            "-F".into(),
            "/dev/null".into(),
            "-i".into(),
            key.display().to_string(),
            "-p".into(),
            self.port.to_string(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "-o".into(),
            "UserKnownHostsFile=/dev/null".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "LogLevel=ERROR".into(),
        ]
    }

    fn ssh(&self, key: &Path, exec: &str) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(self.ssh_args(key))
            .arg("git@127.0.0.1")
            .arg(exec)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    /// Runs `exec` and immediately answers the ref advertisement with a
    /// flush-pkt: "want nothing" for upload-pack, "update nothing" for
    /// receive-pack, so either succeeds for anyone the server let through
    /// without touching the repo (or running the pre-receive hook).
    async fn exec(&self, key: &Path, exec: &str) -> Output {
        let mut child = self.ssh(key, exec).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"0000").await.unwrap();
        drop(stdin);
        tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
            .await
            .unwrap()
            .unwrap()
    }

    async fn git(&self, key: &Path, args: &[&str]) -> Output {
        let ssh_command = format!("ssh {}", self.ssh_args(key).join(" "));
        let fut = Command::new("git")
            .env("GIT_SSH_COMMAND", ssh_command)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        tokio::time::timeout(Duration::from_secs(30), fut)
            .await
            .unwrap()
            .unwrap()
    }

    fn url(&self, path: &str) -> String {
        format!("ssh://git@127.0.0.1:{}/{path}", self.port)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The server's reason goes out as extended data just before the channel
/// failure, but OpenSSH exits on that failure without flushing it, so the
/// refusal itself is all a real client observes. Matching OpenSSH's own
/// wording tells it apart from an auth or transport failure.
fn assert_refused(out: &Output) {
    assert!(!out.status.success());
    assert!(
        stderr(out).contains("exec request failed"),
        "not an exec refusal: {}",
        stderr(out)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_ls_remote_sends_a_quoted_path_that_reaches_the_repo() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("rita", Some(Permission::Reporter)).await;

    let out = server.git(&key, &["ls-remote", &server.url(REPO)]).await;
    assert!(out.status.success(), "ls-remote failed: {}", stderr(&out));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unquoted_path_reaches_the_repo_too() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("rita", Some(Permission::Reporter)).await;

    let out = server.exec(&key, &format!("git-upload-pack /{REPO}")).await;
    assert!(out.status.success(), "upload-pack failed: {}", stderr(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("capabilities^{}"),
        "no ref advertisement in {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quoted_path_with_an_escaped_single_quote_is_refused() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("rita", Some(Permission::Reporter)).await;

    // Unquoting `'\''` can only ever yield a name containing `'`, which no
    // repo name may hold, so refusal is the one outcome a client can see.
    let out = server.exec(&key, r"git-upload-pack '/team/a'\''b'").await;
    assert_refused(&out);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_service_is_refused() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("rita", Some(Permission::Reporter)).await;

    let out = server.exec(&key, "git-upload-archive '/x'").await;
    assert_refused(&out);

    let out = server.exec(&key, "just-a-word").await;
    assert_refused(&out);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reporter_can_fetch_but_not_push() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("rita", Some(Permission::Reporter)).await;

    let out = server.exec(&key, &format!("git-upload-pack '/{REPO}'")).await;
    assert!(out.status.success(), "upload-pack failed: {}", stderr(&out));

    let out = server.exec(&key, &format!("git-receive-pack '/{REPO}'")).await;
    assert_refused(&out);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_developer_can_push() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("dev", Some(Permission::Developer)).await;

    let out = server.exec(&key, &format!("git-receive-pack '/{REPO}'")).await;
    assert!(out.status.success(), "receive-pack failed: {}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("report-status"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_cannot_even_fetch() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("gus", Some(Permission::Guest)).await;

    let out = server.git(&key, &["ls-remote", &server.url(REPO)]).await;
    assert_refused(&out);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_hanging_up_mid_session_leaves_the_server_serving() {
    if !tools_available() {
        return;
    }
    let server = Server::start().await;
    let key = server.add_user("rita", Some(Permission::Reporter)).await;

    // Killed while upload-pack is still waiting for its wants, so the
    // server sees the TCP connection drop rather than a clean close.
    let mut child = server.ssh(&key, &format!("git-upload-pack '/{REPO}'")).spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(30), stdout.read_exact(&mut first))
        .await
        .unwrap()
        .unwrap();
    child.kill().await.unwrap();

    let out = server.git(&key, &["ls-remote", &server.url(REPO)]).await;
    assert!(
        out.status.success(),
        "ls-remote after a hangup failed: {}",
        stderr(&out)
    );
}
