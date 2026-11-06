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
    generate_key_at("0", None)
}

/// `faked_time` is gpg's `--faked-system-time`, so a key can be born (and
/// sign) in the past and already be past its `expire` by the time the test
/// verifies against the real clock.
fn generate_key_at(expire: &str, faked_time: Option<&str>) -> TestKey {
    let homedir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(homedir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let batch = format!(
        "\
%no-protection
Key-Type: eddsa
Key-Curve: ed25519
Subkey-Type: eddsa
Subkey-Curve: ed25519
Name-Real: Test User
Name-Email: test@example.com
Expire-Date: {expire}
%commit
"
    );
    let batch_path = homedir.path().join("batch");
    std::fs::write(&batch_path, batch).unwrap();
    let status = gpg(homedir.path(), faked_time)
        .args(["--no-tty", "--gen-key"])
        .arg(&batch_path)
        .status()
        .unwrap();
    assert!(status.success());

    let armored_public = export(homedir.path());
    TestKey {
        homedir,
        armored_public,
    }
}

fn gpg(homedir: &std::path::Path, faked_time: Option<&str>) -> Command {
    let mut cmd = Command::new("gpg");
    cmd.arg("--homedir").arg(homedir).args(["--batch", "--yes"]);
    if let Some(time) = faked_time {
        cmd.args(["--faked-system-time", time]);
    }
    cmd
}

fn export(homedir: &std::path::Path) -> String {
    let out = gpg(homedir, None)
        .args(["--armor", "--export", "test@example.com"])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

fn sign(key: &TestKey, data: &[u8]) -> String {
    sign_at(key, data, None)
}

fn sign_at(key: &TestKey, data: &[u8], faked_time: Option<&str>) -> String {
    let data_path = key.homedir.path().join("to_sign");
    std::fs::write(&data_path, data).unwrap();
    let out = gpg(key.homedir.path(), faked_time)
        .args(["--armor", "--detach-sign", "--output", "-"])
        .arg(&data_path)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

/// Imports the revocation certificate gpg wrote at key generation and
/// refreshes `armored_public`, so it carries the revocation the way a key
/// re-uploaded by its owner after revoking would.
fn revoke(key: &mut TestKey) {
    let revs = key.homedir.path().join("openpgp-revocs.d");
    let rev_path = std::fs::read_dir(&revs).unwrap().next().unwrap().unwrap().path();
    // gpg ships the certificate with its armor header commented out by a
    // leading ':' so an accidental import can't revoke the key.
    let cert = std::fs::read_to_string(&rev_path)
        .unwrap()
        .replace(":-----BEGIN", "-----BEGIN");
    let cert_path = key.homedir.path().join("revocation.asc");
    std::fs::write(&cert_path, cert).unwrap();
    let status = gpg(key.homedir.path(), None)
        .arg("--import")
        .arg(&cert_path)
        .status()
        .unwrap();
    assert!(status.success());
    key.armored_public = export(key.homedir.path());
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
async fn fingerprint_rejects_a_blob_carrying_two_primary_keys() {
    if !gpg_available() {
        return;
    }
    let mallory = generate_key();
    let alice = generate_key();
    let blob = format!("{}\n{}", mallory.armored_public, alice.armored_public);
    assert!(gitquarry_gpg::fingerprint(&blob).await.is_err());
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

#[tokio::test]
async fn verify_detached_signature_rejects_a_signature_by_an_expired_key() {
    if !gpg_available() {
        return;
    }
    // gpg's own exit status is 0 for this one, so only the status output
    // tells it apart from a live key.
    let key = generate_key_at("1d", Some("20200101T000000!"));
    let data = b"hello release asset";
    let signature = sign_at(&key, data, Some("20200101T010000!"));

    let ok = gitquarry_gpg::verify_detached_signature(data, &signature, &[key.armored_public])
        .await
        .unwrap();
    assert!(!ok);
}

#[tokio::test]
async fn verify_detached_signature_rejects_a_signature_by_a_since_revoked_key() {
    if !gpg_available() {
        return;
    }
    let mut key = generate_key();
    let data = b"hello release asset";
    let signature = sign(&key, data);
    revoke(&mut key);

    let ok = gitquarry_gpg::verify_detached_signature(data, &signature, &[key.armored_public])
        .await
        .unwrap();
    assert!(!ok);
}
