//! `gitquarry` startup as the real binary over a bucket that already has an
//! admin, so it goes straight to serving instead of the first-run wizard.

use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::User;
use gitquarry_core::crypto::bootstrap;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::backend;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PASSPHRASE: &str = "hunter2";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Starting on defaults would quietly drop whatever the operator configured
/// in `.conf/global.json` until the next restart.
#[tokio::test]
async fn an_unreadable_bucket_config_fails_startup_instead_of_using_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let config = dir.path().join("gitquarry.toml");
    std::fs::write(
        &config,
        format!(
            "[buckets.main]\ndriver = \"local\"\nengine = \"redb\"\nbucket = {:?}\npassphrase = {PASSPHRASE:?}\n",
            bucket.to_string_lossy()
        ),
    )
    .unwrap();
    {
        // Dropped before the binary runs: redb holds an exclusive lock.
        let backend = backend(&bucket);
        let cipher = bootstrap::get_or_create_cipher(backend.as_ref(), PASSPHRASE)
            .await
            .unwrap();
        let root = User {
            uid: Uid::new(),
            username: "root".to_string(),
            is_admin: true,
            ..Default::default()
        };
        access::put_user(backend.as_ref(), &cipher, &root, None).await.unwrap();
        backend
            .put_cas(".conf/global.json", b"not a sealed object".to_vec(), None)
            .await
            .unwrap();
    }

    let mut server = Command::new(env!("CARGO_BIN_EXE_gitquarry"))
        .arg("--web-bind")
        .arg(format!("127.0.0.1:{}", free_port()))
        .arg("--ssh-bind")
        .arg(format!("127.0.0.1:{}", free_port()))
        .arg("--mirrors-dir")
        .arg(dir.path().join("mirrors"))
        .env("GITQUARRY_CONFIG", &config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() > deadline {
            server.kill().unwrap();
            server.wait().unwrap();
            break None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let status = status.expect("the server kept running on default settings");
    assert!(!status.success());
}
