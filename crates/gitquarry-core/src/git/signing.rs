//! The instance's own commit-signing key.
//!
//! Every commit this server builds on someone's behalf — a pull request's
//! merge, squash or rebase commit — can be signed, but never with the
//! author's key: the bucket only ever holds the *public* half of a user's
//! GPG key ([`crate::auth::entities::GpgKeyEntry`]), because a server that
//! could sign as you could also forge as you, and the "verified" badge
//! would then be worth nothing. So the instance signs with a key of its
//! own, and the signature says exactly what it can honestly say: this
//! commit was built here.
//!
//! Generated on first use and persisted in the bucket, exactly like
//! [`super::ssh`]'s host key — no key to place, no path to configure, no
//! passphrase to hold. An operator deploys the binary and it has an
//! identity. Every instance sharing a bucket shares the key, which is what
//! makes the signature mean "this deployment" rather than "this container".
//!
//! An SSH key rather than an OpenPGP one, for two reasons that both come
//! down to having no moving parts: `russh::keys` already generates and
//! parses these (the host key uses the same call), and git signs with them
//! natively via `gpg.format=ssh` — no keyring to import into, no
//! `GNUPGHOME`, no agent, no passphrase prompt.
//!
//! **What the signature proves, and what it doesn't.** Anyone holding the
//! bucket passphrase can read this key out and forge commits that appear
//! instance-signed. That is not a new weakness: the same passphrase already
//! lets them rewrite refs outright, which is worse. Read it as "this went
//! through this instance", never as "this person wrote this code" — that
//! second claim is what a *user's* own GPG signature makes, and only they
//! can make it.

use crate::crypto::Cipher;
use crate::storage::{Backend, Cas};
use anyhow::{Context, Result};
use russh::keys::{Algorithm, PrivateKey, ssh_key::LineEnding};
use s3db::Singleton;
use serde::{Deserialize, Serialize};

const SIGNING_KEY: Singleton = Singleton::new(".conf/signing_key.json");

/// Committer name stamped on a commit the instance signs. The *author*
/// stays whoever asked for the merge — same split GitHub uses for its own
/// web-made commits, and the reason this works at all: git matches an SSH
/// signature against the allowed-signers principal for the **committer**
/// address, so that address has to be the one the key belongs to.
pub const INSTANCE_NAME: &str = "GitQuarry";

/// Deliberately in the same synthetic `noreply` domain
/// `domain::pull_request::merger_identity` already uses for an account with
/// no address: it is guaranteed not to collide with a real mailbox, and
/// needs no deploy-time configuration to be stable.
pub const INSTANCE_EMAIL: &str = "instance@noreply.gitquarry";

/// The instance's signing key, both halves.
pub struct InstanceKey {
    /// OpenSSH-armored private key. Written to a mode-0600 temp file for
    /// the length of one `git commit-tree` and nowhere else — git's SSH
    /// signing takes a path, not a key in memory.
    openssh_private: String,
    /// `ssh-ed25519 AAAA...` one-liner, for the allowed-signers file every
    /// verification path needs.
    pub openssh_public: String,
}

impl InstanceKey {
    /// One `allowed_signers` line: the instance address, then its public
    /// key. Git matches the committer's email against the principals here,
    /// which is why [`INSTANCE_EMAIL`] has to be the committer.
    pub fn allowed_signers_line(&self) -> String {
        format!("{INSTANCE_EMAIL} {}\n", self.openssh_public.trim())
    }

    /// Writes the private half to a fresh mode-0600 file and hands back the
    /// handle. Kept as a `NamedTempFile` so it is deleted when the caller
    /// drops it, rather than living in the mirror where a later `git` run —
    /// or anything else reading the mirrors directory — could pick it up.
    pub fn to_temp_file(&self) -> Result<tempfile::NamedTempFile> {
        use std::io::Write;
        let mut file = tempfile::Builder::new()
            .prefix("gitquarry-signing-")
            .tempfile()
            .context("creating a temp file for the signing key")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o600))
                .context("locking down the signing key temp file")?;
        }
        file.write_all(self.openssh_private.as_bytes())
            .context("writing the signing key")?;
        file.flush().context("flushing the signing key")?;
        Ok(file)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SigningKeyDoc {
    #[serde(default)]
    openssh: Option<String>,
}

/// Loads the instance signing key, generating and storing it the first
/// time. Same create-if-absent CAS shape [`super::ssh`]'s host key uses,
/// including its resolution for two instances starting at once: whichever
/// write lands first wins and both then use it, since either key is equally
/// valid — what must not happen is two instances each believing in a
/// different one.
pub async fn load_or_create(backend: &dyn Backend, cipher: &Cipher) -> Result<InstanceKey> {
    if let Some(key) = stored(backend, cipher).await? {
        return Ok(key);
    }

    let key =
        PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).context("generating the instance signing key")?;
    let openssh = key
        .to_openssh(LineEnding::LF)
        .context("serializing the instance signing key")?
        .to_string();
    let doc = SigningKeyDoc { openssh: Some(openssh) };
    match SIGNING_KEY.put(backend, cipher, &doc, None).await? {
        Cas::Ok => from_openssh(&doc.openssh.unwrap_or_default()),
        Cas::Conflict => stored(backend, cipher)
            .await?
            .context("another instance stored a signing key that no longer reads back"),
    }
}

async fn stored(backend: &dyn Backend, cipher: &Cipher) -> Result<Option<InstanceKey>> {
    let (doc, _etag) = SIGNING_KEY.get::<SigningKeyDoc>(backend, cipher).await?;
    doc.openssh.as_deref().map(from_openssh).transpose()
}

fn from_openssh(openssh: &str) -> Result<InstanceKey> {
    let key = PrivateKey::from_openssh(openssh).context("invalid instance signing key in bucket")?;
    let public = key
        .public_key()
        .to_openssh()
        .context("serializing the instance signing key's public half")?;
    Ok(InstanceKey {
        openssh_private: openssh.to_string(),
        openssh_public: public,
    })
}

/// Teaches a mirror to recognise this instance's own signatures, so
/// `%G?` reports `G` for them instead of `N`.
///
/// Git can only verify an SSH signature against a `gpg.ssh.allowedSignersFile`;
/// with none configured it doesn't merely fail the check, it reports the
/// commit as *unsigned* — which every reader of `%G?` then short-circuits
/// on, including the one that decides whether a commit is worth verifying
/// at all. Written into the mirror rather than passed as `-c` on each
/// invocation because the readers are `git log`/`for-each-ref` deep in
/// `browse`, which have no business knowing about signing keys.
///
/// Idempotent and cheap: the file's presence is the marker, so an existing
/// mirror pays one `try_exists` per open. A mirror is a disposable cache
/// (`mirror::gc` evicts it), so losing this costs a rewrite, never
/// correctness of the repository itself.
pub async fn install_allowed_signers(bare_repo: &std::path::Path, key: &InstanceKey) -> Result<()> {
    let allowed = bare_repo.join("gitquarry-allowed-signers");
    if tokio::fs::try_exists(&allowed).await.unwrap_or(false) {
        return Ok(());
    }
    tokio::fs::write(&allowed, key.allowed_signers_line())
        .await
        .with_context(|| format!("writing {}", allowed.display()))?;
    config(bare_repo, "gpg.format", "ssh").await?;
    config(bare_repo, "gpg.ssh.allowedSignersFile", &allowed.to_string_lossy()).await?;
    Ok(())
}

async fn config(bare_repo: &std::path::Path, name: &str, value: &str) -> Result<()> {
    let out = tokio::process::Command::new("git")
        .env("GIT_DIR", bare_repo)
        .args(["config", "--local", "--end-of-options", name, value])
        .output()
        .await
        .context("couldn't run git config")?;
    if !out.status.success() {
        anyhow::bail!(
            "git config {name} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
