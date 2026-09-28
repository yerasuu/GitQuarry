use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::uid::Uid;
use gitquarry_domain::release::*;
use s3db::LocalBackend;
use s3db_crypto::SALT_LEN;
use std::sync::Arc;

fn test_resolved(root: &std::path::Path) -> ResolvedRepo {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[7u8; SALT_LEN]).unwrap());
    let remote = Remote::new(backend, "repos/team/proj".to_string(), cipher);
    ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
    }
}

/// Minimal `AppContext` for [`generate_source_archives`] — only
/// `mirrors_root`/`exe_path`/`locks` actually matter to it (via
/// `open_mirror`); `backend`/`cipher` are never read on this path (the
/// mirror sync reads/writes through `resolved.remote` directly), so an
/// arbitrary throwaway pair is fine.
fn test_ctx(mirrors_root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: mirrors_root.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[19u8; SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: mirrors_root.to_path_buf(),
        config_path: mirrors_root.join("gitquarry.toml"),
        exe_path: mirrors_root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

async fn put_tag(resolved: &ResolvedRepo, tag: &str) {
    let mut manifest = Manifest::default();
    manifest.refs.insert(format!("refs/tags/{tag}"), "a".repeat(40));
    resolved.remote.put_manifest(&manifest, None).await.unwrap();
}

#[tokio::test]
async fn create_rejects_a_nonexistent_tag() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    assert!(
        create(
            &ctx,
            &resolved,
            "v1.0",
            "First",
            "notes",
            false,
            false,
            "alice",
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn create_then_get_roundtrips_and_conflicts_on_a_second_create() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    put_tag(&resolved, "v1.0").await;
    assert_eq!(
        create(
            &ctx,
            &resolved,
            "v1.0",
            "First",
            "notes",
            false,
            false,
            "alice",
            gitquarry_i18n::Locale::Es
        )
        .await
        .unwrap(),
        Cas::Ok
    );
    let (release, etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert!(etag.is_some());
    assert_eq!(release.title, "First");
    assert_eq!(release.created_by, "alice");

    assert_eq!(
        create(
            &ctx,
            &resolved,
            "v1.0",
            "Dup",
            "notes",
            false,
            false,
            "bob",
            gitquarry_i18n::Locale::Es
        )
        .await
        .unwrap(),
        Cas::Conflict
    );
}

#[tokio::test]
async fn edit_requires_an_existing_release() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    assert!(
        edit(
            &resolved,
            "v1.0",
            "New title",
            "new notes",
            false,
            false,
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );

    put_tag(&resolved, "v1.0").await;
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        "alice",
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    assert_eq!(
        edit(
            &resolved,
            "v1.0",
            "New title",
            "new notes",
            false,
            false,
            gitquarry_i18n::Locale::Es
        )
        .await
        .unwrap(),
        Cas::Ok
    );
    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert_eq!(release.title, "New title");
}

#[tokio::test]
async fn create_and_edit_roundtrip_the_prerelease_flag() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    put_tag(&resolved, "v1.0").await;
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        true,
        false,
        "alice",
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert!(release.is_prerelease);

    edit(
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert!(!release.is_prerelease);
}

#[tokio::test]
async fn add_asset_then_remove_asset_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    put_tag(&resolved, "v1.0").await;
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        "alice",
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();

    let src_dir = tempfile::tempdir().unwrap();
    let asset_path = src_dir.path().join("a.bin");
    tokio::fs::write(&asset_path, b"hello").await.unwrap();

    assert_eq!(
        add_asset(
            &ctx,
            &resolved,
            NewAsset {
                tag: "v1.0",
                filename: "a.bin",
                source: &asset_path,
                size: 5,
                sha256: "deadbeef",
                uploaded_by: "alice",
                signature: None,
                locale: gitquarry_i18n::Locale::Es
            }
        )
        .await
        .unwrap(),
        Cas::Ok
    );
    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert_eq!(release.assets.len(), 1);
    assert_eq!(release.assets[0].filename, "a.bin");

    assert!(
        add_asset(
            &ctx,
            &resolved,
            NewAsset {
                tag: "v1.0",
                filename: "a.bin",
                source: &asset_path,
                size: 5,
                sha256: "deadbeef",
                uploaded_by: "alice",
                signature: None,
                locale: gitquarry_i18n::Locale::Es
            }
        )
        .await
        .is_err()
    );

    assert_eq!(
        remove_asset(&resolved, "v1.0", "a.bin", gitquarry_i18n::Locale::Es)
            .await
            .unwrap(),
        Cas::Ok
    );
    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert!(release.assets.is_empty());
    assert!(
        remove_asset(&resolved, "v1.0", "a.bin", gitquarry_i18n::Locale::Es)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn delete_removes_the_release_and_its_assets() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    put_tag(&resolved, "v1.0").await;
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        "alice",
        gitquarry_i18n::Locale::Es,
    )
    .await
    .unwrap();
    delete(&resolved, "v1.0").await.unwrap();
    assert!(resolved.remote.get_release("v1.0").await.unwrap().1.is_none());
}

/// End-to-end through the real path `webui::repo::releases::handler_create_post`
/// spawns: a real bare repo pre-placed exactly where `AppContext::open_mirror`
/// would put it (so the sync step finds everything already local — no S3
/// pack upload/download machinery needed for this test), a real manifest
/// entry for the tag (same as a real push's pre-receive hook would leave),
/// and a real release to attach the generated assets to.
#[tokio::test]
async fn generate_source_archives_uploads_both_as_real_release_assets() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let mirrors_dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(bucket_dir.path());
    let ctx = test_ctx(mirrors_dir.path());

    let git_dir = mirrors_dir.path().join("team/proj");
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .env("GIT_DIR", &git_dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    std::fs::create_dir_all(&git_dir).unwrap();
    run(&["init", "--quiet", "--bare"]);
    // A bare repo has no working tree to commit from directly — a throwaway
    // index + tree write is the plumbing-level way to create a commit
    // straight into a bare repo's object store.
    let blob = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(b"hello\n")?;
            child.wait_with_output()
        })
        .unwrap();
    let blob_sha = String::from_utf8(blob.stdout).unwrap().trim().to_string();
    let mktree = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .args(["mktree"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            writeln!(child.stdin.take().unwrap(), "100644 blob {blob_sha}\tf.txt")?;
            child.wait_with_output()
        })
        .unwrap();
    let tree_sha = String::from_utf8(mktree.stdout).unwrap().trim().to_string();
    let commit = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(["commit-tree", &tree_sha, "-m", "initial"])
        .output()
        .unwrap();
    let commit_sha = String::from_utf8(commit.stdout).unwrap().trim().to_string();

    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/tags/v1.0".to_string(), commit_sha);
    resolved.remote.put_manifest(&manifest, None).await.unwrap();
    create(
        &ctx,
        &resolved,
        "v1.0",
        "First",
        "notes",
        false,
        false,
        "alice",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    generate_source_archives(
        &ctx,
        &resolved,
        "team/proj",
        "v1.0",
        "alice",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let (release, _etag) = resolved.remote.get_release("v1.0").await.unwrap();
    assert_eq!(release.assets.len(), 2);
    let names: Vec<&str> = release.assets.iter().map(|a| a.filename.as_str()).collect();
    assert!(names.contains(&"proj-v1.0.tar.gz"), "{names:?}");
    assert!(names.contains(&"proj-v1.0.zip"), "{names:?}");
    for asset in &release.assets {
        assert!(asset.size > 0);
        assert_eq!(asset.sha256.len(), 64);
    }
}

/// Bare-repo plumbing commit, same trick [`generate_source_archives_uploads_both_as_real_release_assets`]
/// uses — a bare repo has no working tree to commit from directly, so this
/// writes a blob+tree+commit straight into the object store. Every commit
/// reuses the same tree (content is irrelevant here, only the parent chain
/// and subject line matter for [`generate_notes`]'s own commit-range walk).
fn git_commit(git_dir: &std::path::Path, tree_sha: &str, message: &str, seconds: u64, parent: Option<&str>) -> String {
    let mut args = vec!["commit-tree", tree_sha, "-m", message];
    if let Some(parent) = parent {
        args.push("-p");
        args.push(parent);
    }
    // Explicit, strictly increasing dates: `for-each-ref --sort=-creatordate`
    // (what `generate_notes` uses to find "the previous tag") would
    // otherwise order same-second commits ambiguously, since these all run
    // in a tight loop and could land on the same wall-clock second.
    let date = format!("{} +0000", 1_700_000_000 + seconds);
    let out = std::process::Command::new("git")
        .env("GIT_DIR", git_dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_DATE", &date)
        .args(&args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Sets up a real bare repo at `mirrors_dir/team/proj` (same path
/// `AppContext::open_mirror` would use, so it finds everything already
/// local) with a 3-commit linear history and two tags: `v1.0` on the first
/// commit, `v2.0` on the third — real git objects/tags, since
/// [`generate_notes`] walks `git log` ranges, not the S3 manifest.
async fn setup_two_tags(bucket_dir: &std::path::Path, mirrors_dir: &std::path::Path) -> (ResolvedRepo, AppContext) {
    let resolved = test_resolved(bucket_dir);
    let ctx = test_ctx(mirrors_dir);
    let git_dir = mirrors_dir.join("team/proj");
    std::fs::create_dir_all(&git_dir).unwrap();
    let status = std::process::Command::new("git")
        .env("GIT_DIR", &git_dir)
        .args(["init", "--quiet", "--bare"])
        .status()
        .unwrap();
    assert!(status.success());

    // The well-known empty-tree object — always present in every git repo,
    // no need to write it first.
    let tree_sha = "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string();

    let c1 = git_commit(&git_dir, &tree_sha, "first", 1_000, None);
    let c2 = git_commit(&git_dir, &tree_sha, "second", 2_000, Some(&c1));
    let c3 = git_commit(&git_dir, &tree_sha, "third", 3_000, Some(&c2));

    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/tags/v1.0".to_string(), c1);
    manifest.refs.insert("refs/tags/v2.0".to_string(), c3.clone());
    manifest.head = Some("refs/heads/main".to_string());
    manifest.refs.insert("refs/heads/main".to_string(), c3);
    resolved.remote.put_manifest(&manifest, None).await.unwrap();
    (resolved, ctx)
}

#[tokio::test]
async fn generate_notes_rejects_a_nonexistent_tag() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let mirrors_dir = tempfile::tempdir().unwrap();
    let (resolved, ctx) = setup_two_tags(bucket_dir.path(), mirrors_dir.path()).await;
    assert!(
        generate_notes(&ctx, &resolved, "v9.9", gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn generate_notes_lists_only_commits_since_the_previous_tag() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let mirrors_dir = tempfile::tempdir().unwrap();
    let (resolved, ctx) = setup_two_tags(bucket_dir.path(), mirrors_dir.path()).await;

    let notes = generate_notes(&ctx, &resolved, "v2.0", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(notes.contains("second"), "{notes}");
    assert!(notes.contains("third"), "{notes}");
    assert!(!notes.contains("first"), "{notes}");
}

#[tokio::test]
async fn generate_notes_on_the_first_tag_has_no_previous_tag_to_diff_against() {
    let bucket_dir = tempfile::tempdir().unwrap();
    let mirrors_dir = tempfile::tempdir().unwrap();
    let (resolved, ctx) = setup_two_tags(bucket_dir.path(), mirrors_dir.path()).await;

    let notes = generate_notes(&ctx, &resolved, "v1.0", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(notes.contains("first"), "{notes}");
    assert!(!notes.contains("second"), "{notes}");
    assert!(!notes.contains("third"), "{notes}");
}
