//! `gitquarry migrate` run as the real binary against a local bucket.

use std::process::Command;

#[test]
fn a_migration_with_nothing_to_do_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let config = dir.path().join("gitquarry.toml");
    std::fs::write(
        &config,
        format!(
            "[buckets.main]\ndriver = \"local\"\nengine = \"redb\"\nbucket = {:?}\npassphrase = \"hunter2\"\n",
            bucket.to_string_lossy()
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_gitquarry"))
        .arg("migrate")
        .env("GITQUARRY_CONFIG", &config)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // An empty bucket has no grant to index: the pass reports a clean outcome.
    let line = "2 (repo-permission-index): nothing to change";
    assert!(log.contains(line), "missing {line:?} in:\n{log}");
}
