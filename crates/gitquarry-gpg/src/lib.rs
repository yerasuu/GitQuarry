//! Shells out to the system `gpg` binary for OpenPGP key validation and
//! detached-signature verification — same "trust a well-known external
//! tool over reimplementing its crypto" call this codebase already makes
//! for `git` itself (see `git::run`'s own doc comment). Used by release-
//! asset signature verification (`domain::release`); a future commit-
//! signature "Verified" badge would reuse [`verify_detached_signature`]
//! the same way.
//!
//! Every operation runs against a fresh, per-call temp `--homedir` that's
//! discarded when the function returns — nothing here ever touches a
//! shared/system GPG keyring, so concurrent requests (and unrelated users'
//! keys) never interfere with each other, and a request never accidentally
//! trusts a key some other request imported.

use anyhow::{Context, Result, bail};
use std::path::Path;
use tokio::process::Command;

/// Same tempdir every call here starts from — `0700` explicitly rather
/// than relying on the process umask, since `gpg` itself warns (and some
/// distro builds refuse to operate at all) on a homedir any other user on
/// the box could read.
fn scratch_homedir() -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir().context("creating a scratch gpg homedir")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .context("setting scratch gpg homedir permissions")?;
    }
    Ok(dir)
}

async fn import(homedir: &Path, armored_key: &str) -> Result<()> {
    let key_path = homedir.join("key.asc");
    tokio::fs::write(&key_path, armored_key)
        .await
        .context("writing a scratch gpg key file")?;
    let out = Command::new("gpg")
        .arg("--homedir")
        .arg(homedir)
        .args(["--batch", "--yes", "--no-tty", "--import"])
        .arg(&key_path)
        .output()
        .await
        .context("couldn't run gpg --import")?;
    if !out.status.success() {
        bail!(
            "not a valid OpenPGP public key: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Imports `armored_key` into a scratch keyring and returns its primary
/// key's fingerprint (40 hex chars) — `Err` if it isn't a well-formed
/// OpenPGP public key block. Used both to validate a key at add-time
/// (`auth::domain::user::add_gpg_key`) and to show/remove-by-fingerprint
/// the same way SSH pubkeys already work (`auth::access::key_fingerprint`,
/// a pure function — unlike that one, this needs `gpg` itself: there's no
/// pure-Rust OpenPGP packet parsing here to compute one otherwise).
pub async fn fingerprint(armored_key: &str) -> Result<String> {
    let dir = scratch_homedir()?;
    import(dir.path(), armored_key).await?;
    let out = Command::new("gpg")
        .arg("--homedir")
        .arg(dir.path())
        .args(["--batch", "--no-tty", "--with-colons", "--list-keys"])
        .output()
        .await
        .context("couldn't run gpg --list-keys")?;
    if !out.status.success() {
        bail!(
            "gpg --list-keys failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // `fpr:::::::::<fingerprint>:` — the first `fpr` record belongs to the
    // primary key just imported (a fresh homedir has nothing else in it).
    text.lines()
        .find(|l| l.starts_with("fpr:"))
        .and_then(|l| l.split(':').nth(9))
        .map(str::to_string)
        .context("gpg --list-keys returned no fingerprint")
}

/// `Ok(true)` if `signature` is a valid detached OpenPGP signature over
/// `data` made by one of `armored_keys`. Never `Err` for "the signature
/// doesn't verify" — only for an operational failure (couldn't spawn
/// `gpg`, couldn't write the scratch files); a bad/mismatched signature,
/// or an unparseable one, is `Ok(false)`, same "a failed check is a normal
/// outcome, not an error" convention `password::verify_password` already
/// follows. A key in `armored_keys` that fails to import (garbled/expired)
/// is skipped rather than failing the whole call — it simply can't verify
/// anything, same as if it weren't offered at all.
pub async fn verify_detached_signature(data: &[u8], signature: &str, armored_keys: &[String]) -> Result<bool> {
    if armored_keys.is_empty() {
        return Ok(false);
    }
    let dir = scratch_homedir()?;
    for key in armored_keys {
        let _ = import(dir.path(), key).await;
    }
    let data_path = dir.path().join("data");
    let sig_path = dir.path().join("data.sig");
    tokio::fs::write(&data_path, data)
        .await
        .context("writing scratch signed-data file")?;
    tokio::fs::write(&sig_path, signature)
        .await
        .context("writing scratch signature file")?;
    let out = Command::new("gpg")
        .arg("--homedir")
        .arg(dir.path())
        .args(["--batch", "--no-tty", "--verify"])
        .arg(&sig_path)
        .arg(&data_path)
        .output()
        .await
        .context("couldn't run gpg --verify")?;
    Ok(out.status.success())
}
