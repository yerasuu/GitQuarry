//! `git::browse::commit_raw_signature` — extracts a signed commit's
//! detached signature, verified end-to-end against a real `gpg`-signed
//! commit. Skipped (not failed) if `gpg` isn't on `PATH`.

use gitquarry_core::git;

fn gpg_available() -> bool {
    std::process::Command::new("gpg").arg("--version").output().is_ok()
}

struct SignedRepo {
    _work: tempfile::TempDir,
    _gpg_homedir: tempfile::TempDir,
    git_dir: std::path::PathBuf,
    signed_sha: String,
    unsigned_sha: String,
    armored_public: String,
}

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
    run(&["commit", "--quiet", "-m", "unsigned commit"], false);
    let unsigned_sha = String::from_utf8(
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

    std::fs::write(work.path().join("a.txt"), "hello again\n").unwrap();
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
        unsigned_sha,
        armored_public,
    }
}

#[tokio::test]
async fn unsigned_commit_has_no_signature() {
    if !gpg_available() {
        return;
    }
    let repo = setup_signed_repo();
    let result = git::browse::commit_raw_signature(&repo.git_dir, &repo.unsigned_sha)
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn signed_commit_extracts_a_signature_that_verifies_against_the_signing_key() {
    if !gpg_available() {
        return;
    }
    let repo = setup_signed_repo();
    let (payload, signature) = git::browse::commit_raw_signature(&repo.git_dir, &repo.signed_sha)
        .await
        .unwrap()
        .expect("signed commit should carry a gpgsig header");
    assert!(signature.contains("BEGIN PGP SIGNATURE"), "{signature}");

    let ok = gitquarry_gpg::verify_detached_signature(&payload, &signature, std::slice::from_ref(&repo.armored_public))
        .await
        .unwrap();
    assert!(ok);

    // Tampering with the extracted payload (as if the commit's own content
    // had been altered) must make it stop verifying — proof the extraction
    // really reproduces the exact bytes that were signed, not just
    // something signature-shaped.
    let mut tampered = payload.clone();
    tampered.extend_from_slice(b"extra");
    let ok = gitquarry_gpg::verify_detached_signature(&tampered, &signature, &[repo.armored_public])
        .await
        .unwrap();
    assert!(!ok);
}
