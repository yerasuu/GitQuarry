//! Exercises the real `gpg` binary — generates a throwaway keypair, signs
//! data with it, and checks `gitquarry_gpg`'s own verification
//! against that real signature. Skipped (not failed) if `gpg` isn't on
//! `PATH`, same "an optional external tool missing skips rather than
//! fails" tolerance other environment-dependent tests in this workspace
//! already have.

use std::process::Command;

struct TestKey {
    homedir: tempfile::TempDir,
    armored_public: String,
}

fn gpg_available() -> bool {
    Command::new("gpg").arg("--version").output().is_ok()
}

fn generate_key() -> TestKey {
    let homedir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(homedir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let batch = "\
%no-protection
Key-Type: eddsa
Key-Curve: ed25519
Subkey-Type: eddsa
Subkey-Curve: ed25519
Name-Real: Test User
Name-Email: test@example.com
Expire-Date: 0
%commit
";
    let batch_path = homedir.path().join("batch");
    std::fs::write(&batch_path, batch).unwrap();
    let status = Command::new("gpg")
        .arg("--homedir")
        .arg(homedir.path())
        .args(["--batch", "--yes", "--no-tty", "--gen-key"])
        .arg(&batch_path)
        .status()
        .unwrap();
    assert!(status.success());

    let out = Command::new("gpg")
        .arg("--homedir")
        .arg(homedir.path())
        .args(["--batch", "--armor", "--export", "test@example.com"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let armored_public = String::from_utf8(out.stdout).unwrap();
    TestKey {
        homedir,
        armored_public,
    }
}

fn sign(key: &TestKey, data: &[u8]) -> String {
    let data_path = key.homedir.path().join("to_sign");
    std::fs::write(&data_path, data).unwrap();
    let out = Command::new("gpg")
        .arg("--homedir")
        .arg(key.homedir.path())
        .args(["--batch", "--yes", "--armor", "--detach-sign", "--output", "-"])
        .arg(&data_path)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
async fn fingerprint_extracts_a_real_key_fingerprint() {
    if !gpg_available() {
        return;
    }
    let key = generate_key();
    let fp = gitquarry_gpg::fingerprint(&key.armored_public).await.unwrap();
    assert_eq!(fp.len(), 40, "{fp}");
    assert!(fp.chars().all(|c| c.is_ascii_hexdigit()), "{fp}");
}

#[tokio::test]
async fn fingerprint_rejects_garbage() {
    if !gpg_available() {
        return;
    }
    assert!(gitquarry_gpg::fingerprint("not a key").await.is_err());
}

#[tokio::test]
async fn verify_detached_signature_accepts_a_real_signature_by_a_known_key() {
    if !gpg_available() {
        return;
    }
    let key = generate_key();
    let data = b"hello release asset";
    let signature = sign(&key, data);

    let ok = gitquarry_gpg::verify_detached_signature(data, &signature, std::slice::from_ref(&key.armored_public))
        .await
        .unwrap();
    assert!(ok);
}

#[tokio::test]
async fn verify_detached_signature_rejects_a_signature_by_an_unrelated_key() {
    if !gpg_available() {
        return;
    }
    let signer = generate_key();
    let other = generate_key();
    let data = b"hello release asset";
    let signature = sign(&signer, data);

    let ok = gitquarry_gpg::verify_detached_signature(data, &signature, std::slice::from_ref(&other.armored_public))
        .await
        .unwrap();
    assert!(!ok);
}

#[tokio::test]
async fn verify_detached_signature_rejects_tampered_data() {
    if !gpg_available() {
        return;
    }
    let key = generate_key();
    let signature = sign(&key, b"original content");

    let ok = gitquarry_gpg::verify_detached_signature(b"tampered content", &signature, &[key.armored_public])
        .await
        .unwrap();
    assert!(!ok);
}

#[tokio::test]
async fn verify_detached_signature_is_false_with_no_keys_offered() {
    if !gpg_available() {
        return;
    }
    let ok = gitquarry_gpg::verify_detached_signature(b"data", "not even a real signature", &[])
        .await
        .unwrap();
    assert!(!ok);
}
