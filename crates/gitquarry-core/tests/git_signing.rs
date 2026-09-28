//! The instance signing key's persistence contract.
//!
//! It has to survive a restart, and every instance sharing a bucket has to
//! agree on it. A key regenerated per process would be worse than no
//! signing at all: every signature made before the restart would stop
//! verifying, silently, and a reader would see previously-good commits turn
//! unverified for no visible reason.

use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::git::signing;
use gitquarry_core::storage::local::LocalBackend;

fn backend_and_cipher(root: &std::path::Path) -> (LocalBackend, Cipher) {
    let backend = LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    });
    let cipher = Cipher::derive("test passphrase", &[31u8; SALT_LEN]).unwrap();
    (backend, cipher)
}

/// Generated once, then read back — not re-minted on the next call, which
/// is what a process restart looks like from here.
#[tokio::test]
async fn the_key_is_generated_once_and_then_reused() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = backend_and_cipher(dir.path());

    let first = signing::load_or_create(&backend, &cipher).await.unwrap();
    let second = signing::load_or_create(&backend, &cipher).await.unwrap();

    assert_eq!(first.openssh_public, second.openssh_public);
    assert!(
        first.openssh_public.starts_with("ssh-ed25519 "),
        "{}",
        first.openssh_public
    );
}

/// It lives in the bucket, not on local disk, so a second process — another
/// container, or the same one after a restart with nothing persisted
/// locally — picks up the identity that already exists rather than
/// inventing a rival one.
#[tokio::test]
async fn a_second_instance_on_the_same_bucket_gets_the_same_key() {
    let dir = tempfile::tempdir().unwrap();
    let (backend_a, cipher_a) = backend_and_cipher(dir.path());
    let first = signing::load_or_create(&backend_a, &cipher_a).await.unwrap();
    // redb allows only one open Database handle per file per process, so the
    // first must close before the second opens — otherwise this second
    // "instance" can't stand in for a genuinely separate process.
    drop(backend_a);

    // A separate backend handle over the same bucket: no shared in-memory
    // state, exactly what a second deployment reads through.
    let (backend_b, cipher_b) = backend_and_cipher(dir.path());
    let second = signing::load_or_create(&backend_b, &cipher_b).await.unwrap();

    assert_eq!(first.openssh_public, second.openssh_public);
}

/// The flip side, and the reason a dev bucket and a production one keep
/// separate identities without anyone configuring that: the key belongs to
/// the bucket, so different buckets sign as different instances.
#[tokio::test]
async fn a_different_bucket_gets_a_different_key() {
    let dev = tempfile::tempdir().unwrap();
    let prod = tempfile::tempdir().unwrap();
    let (dev_backend, dev_cipher) = backend_and_cipher(dev.path());
    let (prod_backend, prod_cipher) = backend_and_cipher(prod.path());

    let dev_key = signing::load_or_create(&dev_backend, &dev_cipher).await.unwrap();
    let prod_key = signing::load_or_create(&prod_backend, &prod_cipher).await.unwrap();

    assert_ne!(dev_key.openssh_public, prod_key.openssh_public);
}

/// The allowed-signers line git checks a signature against: the instance
/// address, then the public half. Nothing else — a stray extra principal
/// there would let another address' signatures pass as the instance's.
#[tokio::test]
async fn the_allowed_signers_line_names_the_instance_address_and_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let (backend, cipher) = backend_and_cipher(dir.path());
    let key = signing::load_or_create(&backend, &cipher).await.unwrap();

    let line = key.allowed_signers_line();

    assert!(line.starts_with(signing::INSTANCE_EMAIL), "{line}");
    assert!(line.contains(key.openssh_public.trim()), "{line}");
    assert_eq!(line.lines().count(), 1, "{line}");
}
