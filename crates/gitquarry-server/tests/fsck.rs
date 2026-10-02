//! `gitquarry fsck` run as the real binary against a local bucket, so what is
//! checked is what an operator gets: which objects `--apply` deletes and
//! which it must leave alone.

use gitquarry_test_support::backend;
use std::path::Path;
use std::process::Command;

const KNOWN: [&str; 4] = [
    "repos/a/refs.json",
    "repos/a/.conf/config.json",
    "repos/a/objects/pack/pack-abc123.pack",
    "repos/a/objects/pack/pack-abc123.idx",
];
const MARKERS: [&str; 2] = ["repos/a/objects/", "repos/a/.conf/"];
const UNEXPECTED: [&str; 2] = ["repos/a/README.md", "repos/a/objects/pack/junk.txt"];

async fn present(bucket: &Path) -> Vec<String> {
    let b = backend(bucket);
    let mut keys = Vec::with_capacity(KNOWN.len() + MARKERS.len() + UNEXPECTED.len());
    for key in KNOWN.iter().chain(&MARKERS).chain(&UNEXPECTED) {
        // An existing key always carries an etag, even a zero-byte placeholder.
        if b.get(key).await.unwrap().1.is_some() {
            keys.push(key.to_string());
        }
    }
    keys
}

fn fsck(config: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_gitquarry"))
        .arg("fsck")
        .args(args)
        .env("GITQUARRY_CONFIG", config)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test]
async fn apply_deletes_only_folder_placeholders_and_never_unexpected_data() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    {
        // Dropped before the binary runs: redb holds an exclusive lock.
        let b = backend(&bucket);
        for key in KNOWN.iter().chain(&UNEXPECTED) {
            b.put_cas(key, b"data".to_vec(), None).await.unwrap();
        }
        for key in MARKERS {
            b.put_cas(key, Vec::new(), None).await.unwrap();
        }
    }
    let config = dir.path().join("gitquarry.toml");
    std::fs::write(
        &config,
        format!(
            "[buckets.main]\ndriver = \"local\"\nengine = \"redb\"\nbucket = {:?}\npassphrase = \"hunter2\"\n",
            bucket.to_string_lossy()
        ),
    )
    .unwrap();

    let everything: Vec<String> = KNOWN
        .iter()
        .chain(&MARKERS)
        .chain(&UNEXPECTED)
        .map(|k| k.to_string())
        .collect();

    fsck(&config, &[]);
    assert_eq!(present(&bucket).await, everything, "a dry run deletes nothing");

    fsck(&config, &["--apply"]);
    let kept: Vec<String> = KNOWN.iter().chain(&UNEXPECTED).map(|k| k.to_string()).collect();
    assert_eq!(present(&bucket).await, kept);
}
