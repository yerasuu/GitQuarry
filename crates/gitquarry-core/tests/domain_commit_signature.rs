//! `domain::commit_signature::verified_signer` and the signing-epoch stamp
//! that keys its cache alongside the sha. A verdict depends on which
//! account claims the commit's author email and which GPG keys that
//! account has registered — both mutable — so caching by sha alone made
//! the answer stick forever: a key registered *after* first render never
//! took effect, and a key removed after one never stopped vouching.
//!
//! The `gpg`-backed tests are skipped (not failed) if `gpg` isn't on
//! `PATH`, same as `git_browse_signature.rs`, whose fixture shape this
//! reuses.

use gitquarry_core::auth::domain::user;
use gitquarry_core::domain::commit_signature::{CommitSigner, verified_signer};
use gitquarry_core::storage::commit_sig::{bump_signing_epoch, signing_epoch};
use gitquarry_i18n::Locale;
use gitquarry_test_support::{account, ctx};

fn gpg_available() -> bool {
    std::process::Command::new("gpg").arg("--version").output().is_ok()
}

struct SignedRepo {
    _work: tempfile::TempDir,
    _gpg_homedir: tempfile::TempDir,
    git_dir: std::path::PathBuf,
    signed_sha: String,
    armored_public: String,
}

/// One repo with a single `gpg`-signed commit authored by
/// `test@example.com`, plus that key's armored public half — everything
/// `verified_signer` needs to reach a real verdict instead of a stub.
fn setup_signed_repo() -> SignedRepo {
    let gpg_homedir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(gpg_homedir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let batch = "\
%no-protection
Key-Type: eddsa
Key-Curve: ed25519
Name-Real: Test Author
Name-Email: test@example.com
Expire-Date: 0
%commit
";
    let batch_path = gpg_homedir.path().join("batch");
    std::fs::write(&batch_path, batch).unwrap();
    let status = std::process::Command::new("gpg")
        .arg("--homedir")
        .arg(gpg_homedir.path())
        .args(["--batch", "--yes", "--no-tty", "--gen-key"])
        .arg(&batch_path)
        .status()
        .unwrap();
    assert!(status.success());
    let out = std::process::Command::new("gpg")
        .arg("--homedir")
        .arg(gpg_homedir.path())
        .args(["--batch", "--armor", "--export", "test@example.com"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let armored_public = String::from_utf8(out.stdout).unwrap();

    let work = tempfile::tempdir().unwrap();
    let run = |args: &[&str], gnupghome: bool| {
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(work.path())
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", "test@example.com");
        if gnupghome {
            cmd.env("GNUPGHOME", gpg_homedir.path());
        }
        let status = cmd.args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };

    run(&["init", "--quiet", "--initial-branch=main"], false);
    std::fs::write(work.path().join("a.txt"), "hello\n").unwrap();
    run(&["add", "."], false);
    run(
        &[
            "-c",
            "user.signingkey=test@example.com",
            "-c",
            "gpg.program=gpg",
            "commit",
            "--quiet",
            "-S",
            "-m",
            "signed commit",
        ],
        true,
    );
    let signed_sha = String::from_utf8(
        std::process::Command::new("git")
            .current_dir(work.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let git_dir = work.path().join(".git");
    SignedRepo {
        _work: work,
        _gpg_homedir: gpg_homedir,
        git_dir,
        signed_sha,
        armored_public,
    }
}

#[tokio::test]
async fn signing_epoch_starts_at_zero_and_every_bump_changes_it() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let backend = ctx.backend();

    assert_eq!(signing_epoch(backend.as_ref(), &ctx.cipher).await.unwrap(), 0);
    bump_signing_epoch(backend.as_ref(), &ctx.cipher).await;
    assert_eq!(signing_epoch(backend.as_ref(), &ctx.cipher).await.unwrap(), 1);
    bump_signing_epoch(backend.as_ref(), &ctx.cipher).await;
    assert_eq!(signing_epoch(backend.as_ref(), &ctx.cipher).await.unwrap(), 2);
}

/// The regression that made this whole epoch stamp necessary: the verdict
/// is computed (and cached) before the key exists, then the key is
/// registered. Without invalidation the cached `None` wins forever and the
/// commit stays "unverified" no matter what the account does.
#[tokio::test]
async fn registering_a_key_after_a_miss_makes_the_commit_verify() {
    if !gpg_available() {
        return;
    }
    let repo = setup_signed_repo();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let backend = ctx.backend();
    account(&ctx, "alice", Some("test@example.com")).await;

    // Cold: the account exists and claims the email, but has no key yet.
    let before = verified_signer(
        &backend,
        &ctx.cipher,
        &repo.git_dir,
        &repo.signed_sha,
        'G',
        "test@example.com",
    )
    .await
    .unwrap();
    assert_eq!(before, None);

    user::add_gpg_key(&ctx, "alice", &repo.armored_public, Locale::En)
        .await
        .unwrap();

    let after = verified_signer(
        &backend,
        &ctx.cipher,
        &repo.git_dir,
        &repo.signed_sha,
        'G',
        "test@example.com",
    )
    .await
    .unwrap();
    assert_eq!(after, Some(CommitSigner::User("alice".to_string())));
}

/// The other half, and the one that actually matters for trust: a key
/// removed from the account must stop vouching for commits it already
/// verified, rather than leaving a "verified" badge behind indefinitely.
#[tokio::test]
async fn removing_a_key_stops_it_vouching_for_an_already_verified_commit() {
    if !gpg_available() {
        return;
    }
    let repo = setup_signed_repo();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let backend = ctx.backend();
    account(&ctx, "alice", Some("test@example.com")).await;
    user::add_gpg_key(&ctx, "alice", &repo.armored_public, Locale::En)
        .await
        .unwrap();

    let verified = verified_signer(
        &backend,
        &ctx.cipher,
        &repo.git_dir,
        &repo.signed_sha,
        'G',
        "test@example.com",
    )
    .await
    .unwrap();
    assert_eq!(verified, Some(CommitSigner::User("alice".to_string())));

    let fingerprint = gitquarry_gpg::fingerprint(&repo.armored_public).await.unwrap();
    user::remove_gpg_key(&ctx, "alice", &fingerprint, Locale::En)
        .await
        .unwrap();

    let after = verified_signer(
        &backend,
        &ctx.cipher,
        &repo.git_dir,
        &repo.signed_sha,
        'G',
        "test@example.com",
    )
    .await
    .unwrap();
    assert_eq!(after, None);
}

/// `'N'` (unsigned) short-circuits before the cache and before any `gpg`
/// spawn, so it costs nothing and never writes an entry — unchanged by the
/// epoch stamp.
#[tokio::test]
async fn an_unsigned_commit_short_circuits_without_touching_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let backend = ctx.backend();

    let signer = verified_signer(
        &backend,
        &ctx.cipher,
        std::path::Path::new("/nonexistent"),
        "0000000000000000000000000000000000000000",
        'N',
        "nobody@example.com",
    )
    .await
    .unwrap();
    assert_eq!(signer, None);
}
