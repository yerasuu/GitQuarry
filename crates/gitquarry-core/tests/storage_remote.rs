use gitquarry_core::storage::remote::*;
use gitquarry_core::storage::{Backend, Cas};
use gitquarry_test_support::{backend, cipher};
use std::sync::Arc;

/// A `Remote` over `backend`, scoped to `prefix` — end-to-end exercise of
/// the streaming pipeline (`Cipher::seal_stream`/`open_stream` +
/// `Backend::put_stream`/`get_stream`) through the one driver this sandbox
/// can actually run, same spirit as a real push/clone against
/// `driver = "local"`. Takes a backend rather than a root because redb allows
/// only one open handle per file per process, so two `Remote`s on the same
/// bucket must share one.
fn test_remote(backend: &Arc<dyn Backend>, prefix: &str) -> Remote {
    Remote::new(backend.clone(), prefix.to_string(), cipher())
}

#[tokio::test]
async fn upload_pack_then_download_pack_roundtrips_through_local_backend() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let backend = backend(bucket_dir.path());
    let remote = test_remote(&backend, "repos/team/proj");

    let src_dir = tempfile::tempdir().unwrap();
    let pack_path = src_dir.path().join("pack-abc.pack");
    let idx_path = src_dir.path().join("pack-abc.idx");
    // Multi-chunk-sized content (bigger than the crypto layer's own
    // chunk size) so this actually exercises the streaming path, not
    // just a single tiny chunk.
    let pack_bytes: Vec<u8> = (0..(4 * 1024 * 1024 + 777)).map(|i| (i % 256) as u8).collect();
    let idx_bytes = b"a small idx".to_vec();
    tokio::fs::write(&pack_path, &pack_bytes).await.unwrap();
    tokio::fs::write(&idx_path, &idx_bytes).await.unwrap();

    remote.upload_pack("pack-abc", &pack_path, &idx_path).await.unwrap();

    let dest_dir = tempfile::tempdir().unwrap();
    remote.download_pack("pack-abc", dest_dir.path()).await.unwrap();

    assert_eq!(
        tokio::fs::read(dest_dir.path().join("pack-abc.pack")).await.unwrap(),
        pack_bytes
    );
    assert_eq!(
        tokio::fs::read(dest_dir.path().join("pack-abc.idx")).await.unwrap(),
        idx_bytes
    );
}

#[tokio::test]
async fn upload_lfs_object_then_download_roundtrips_through_local_backend() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let backend = backend(bucket_dir.path());
    let remote = test_remote(&backend, "repos/team/proj");
    let oid = "a".repeat(64);

    assert!(!remote.lfs_object_exists(&oid).await.unwrap());

    let src_dir = tempfile::tempdir().unwrap();
    let src_path = src_dir.path().join("blob.bin");
    let bytes: Vec<u8> = (0..(2 * 1024 * 1024 + 123)).map(|i| (i % 256) as u8).collect();
    tokio::fs::write(&src_path, &bytes).await.unwrap();

    remote.upload_lfs_object(&oid, &src_path).await.unwrap();
    assert!(remote.lfs_object_exists(&oid).await.unwrap());

    let dest_path = src_dir.path().join("downloaded.bin");
    remote.download_lfs_object(&oid, &dest_path).await.unwrap();
    assert_eq!(tokio::fs::read(&dest_path).await.unwrap(), bytes);
}

#[tokio::test]
async fn release_and_asset_roundtrip_and_delete_cleanly() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let backend = backend(bucket_dir.path());
    let remote = test_remote(&backend, "repos/team/proj");

    assert_eq!(remote.get_release("v1.0").await.unwrap().1, None);
    assert_eq!(remote.list_release_tags().await.unwrap(), Vec::<String>::new());

    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "First release".to_string(),
        notes: "notes here".to_string(),
        assets: Vec::new(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: false,
    };
    assert_eq!(remote.put_release(&release, None).await.unwrap(), Cas::Ok);
    // Creating again without the fresh etag must not silently overwrite.
    assert_eq!(remote.put_release(&release, None).await.unwrap(), Cas::Conflict);

    let (fetched, etag) = remote.get_release("v1.0").await.unwrap();
    assert_eq!(fetched.title, "First release");
    assert_eq!(remote.list_release_tags().await.unwrap(), vec!["v1.0".to_string()]);

    let src_dir = tempfile::tempdir().unwrap();
    let asset_path = src_dir.path().join("build.tar.gz");
    let asset_bytes = b"pretend release archive".to_vec();
    tokio::fs::write(&asset_path, &asset_bytes).await.unwrap();
    remote.upload_asset("v1.0", "build.tar.gz", &asset_path).await.unwrap();

    let dest_path = src_dir.path().join("downloaded.tar.gz");
    remote.download_asset("v1.0", "build.tar.gz", &dest_path).await.unwrap();
    assert_eq!(tokio::fs::read(&dest_path).await.unwrap(), asset_bytes);

    // A second, unrelated tag whose name is a string-prefix of this one
    // must never be touched by this tag's delete.
    let other = ReleaseMeta {
        tag: "v1.0.1".to_string(),
        ..release
    };
    remote.put_release(&other, None).await.unwrap();

    assert!(etag.is_some());

    remote.delete_release("v1.0").await.unwrap();
    assert_eq!(remote.get_release("v1.0").await.unwrap().1, None);
    assert!(remote.download_asset("v1.0", "build.tar.gz", &dest_path).await.is_err());
    // The other tag (a string-superset of "v1.0") survives untouched.
    assert!(remote.get_release("v1.0.1").await.unwrap().1.is_some());
}

#[tokio::test]
async fn copy_all_to_reencrypts_a_pack_into_a_new_prefix() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let backend = backend(bucket_dir.path());
    let src_remote = test_remote(&backend, "repos/alice/proj");
    let dest_remote = test_remote(&backend, "repos/bob/proj");

    let src_dir = tempfile::tempdir().unwrap();
    let pack_path = src_dir.path().join("pack-xyz.pack");
    let idx_path = src_dir.path().join("pack-xyz.idx");
    let pack_bytes: Vec<u8> = (0..(2 * 1024 * 1024 + 42)).map(|i| (i * 7 % 256) as u8).collect();
    tokio::fs::write(&pack_path, &pack_bytes).await.unwrap();
    tokio::fs::write(&idx_path, b"idx").await.unwrap();
    src_remote.upload_pack("pack-xyz", &pack_path, &idx_path).await.unwrap();

    src_remote.copy_all_to(&dest_remote).await.unwrap();

    let dest_dir = tempfile::tempdir().unwrap();
    dest_remote.download_pack("pack-xyz", dest_dir.path()).await.unwrap();
    assert_eq!(
        tokio::fs::read(dest_dir.path().join("pack-xyz.pack")).await.unwrap(),
        pack_bytes
    );
}

/// `copy_all_to` has to re-encrypt every object under a repo's prefix,
/// not just packs (sealed with `Cipher::seal_stream`) — `refs.json`,
/// `config.json`, and every other small JSON entity are sealed with the
/// plain, single-shot `Cipher::seal` instead, a genuinely different
/// on-disk format `open_stream` can't read (fails "not a gitquarry
/// stream"). This is exactly what a real fork/move copies alongside a
/// repo's packs, so this test fails loudly if that regresses.
#[tokio::test]
async fn copy_all_to_reencrypts_a_plain_sealed_manifest_too() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let backend = backend(bucket_dir.path());
    let src_remote = test_remote(&backend, "repos/alice/proj");
    let dest_remote = test_remote(&backend, "repos/bob/proj");

    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), "a".repeat(40));
    manifest.head = Some("refs/heads/main".to_string());
    src_remote.put_manifest(&manifest, None).await.unwrap();

    src_remote.copy_all_to(&dest_remote).await.unwrap();

    let (copied, etag) = dest_remote.get_manifest().await.unwrap();
    assert!(etag.is_some());
    assert_eq!(copied, manifest);
}

/// Both formats can coexist under the same repo prefix in one
/// `copy_all_to` call — a pack (stream-sealed) alongside a manifest
/// (plain-sealed) — and each has to come out re-encrypted correctly on
/// its own, not just whichever format happens to run first.
#[tokio::test]
async fn copy_all_to_handles_mixed_stream_and_plain_sealed_objects_together() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let backend = backend(bucket_dir.path());
    let src_remote = test_remote(&backend, "repos/alice/proj");
    let dest_remote = test_remote(&backend, "repos/bob/proj");

    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/heads/main".to_string(), "b".repeat(40));
    src_remote.put_manifest(&manifest, None).await.unwrap();

    let src_dir = tempfile::tempdir().unwrap();
    let pack_path = src_dir.path().join("pack-abc.pack");
    let idx_path = src_dir.path().join("pack-abc.idx");
    tokio::fs::write(&pack_path, b"pretend pack bytes").await.unwrap();
    tokio::fs::write(&idx_path, b"pretend idx bytes").await.unwrap();
    src_remote.upload_pack("pack-abc", &pack_path, &idx_path).await.unwrap();

    src_remote.copy_all_to(&dest_remote).await.unwrap();

    let (copied_manifest, _etag) = dest_remote.get_manifest().await.unwrap();
    assert_eq!(copied_manifest, manifest);

    let dest_dir = tempfile::tempdir().unwrap();
    dest_remote.download_pack("pack-abc", dest_dir.path()).await.unwrap();
    assert_eq!(
        tokio::fs::read(dest_dir.path().join("pack-abc.pack")).await.unwrap(),
        b"pretend pack bytes"
    );
}
