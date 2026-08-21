use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::git;
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::remote::{AssetMeta, Manifest, ReleaseMeta, Remote};
use gitquarry_core::storage::uid::Uid;
use gitquarry_webui::webui::repo::releases::*;
use std::sync::Arc;

fn test_resolved(root: &std::path::Path) -> ResolvedRepo {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[11u8; SALT_LEN]).unwrap());
    let remote = Remote::new(backend, "repos/team/proj".to_string(), cipher);
    ResolvedRepo {
        uid: Uid::from("team/proj".to_string()),
        remote,
    }
}

/// Minimal `AppContext` for these tests — `can_write_releases` only
/// needs `backend`/`cipher` (and only reaches them for a session cookie
/// that's never present in an anonymous test `HeaderMap`, so it never
/// touches the backend at all here), same as `render_star_button`'s own
/// untested-for-the-logged-in-case precedent elsewhere in this codebase.
fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[11u8; SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

#[tokio::test]
async fn releases_list_shows_hint_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    let html = render_releases_list(&ctx, &resolved, "team/proj", &HeaderMap::new())
        .await
        .unwrap();
    assert!(html.contains("no releases yet"));
}

#[tokio::test]
async fn anonymous_viewer_never_sees_the_generate_notes_button_or_a_prefill() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    let prefill = ReleaseFormPrefill {
        tag: "v1.0".to_string(),
        title: "leaked title".to_string(),
        notes: "leaked notes text".to_string(),
        is_prerelease: false,
        is_draft: false,
    };
    let html = render_releases_list_with_prefill(&ctx, &resolved, "team/proj", &HeaderMap::new(), Some(&prefill), None)
        .await
        .unwrap();
    // `can_write_releases` is false for an anonymous session — the whole
    // create form (prefill included) stays behind that gate, same as it
    // would for any other viewer without write access.
    assert!(!html.contains("generate notes from commits"), "{html}");
    assert!(!html.contains("leaked title"), "{html}");
    assert!(!html.contains("leaked notes text"), "{html}");
}

#[tokio::test]
async fn releases_list_shows_each_release_linked_by_tag() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "First release".to_string(),
        notes: String::new(),
        assets: Vec::new(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: false,
    };
    resolved.remote.put_release(&release, None).await.unwrap();

    let html = render_releases_list(&ctx, &resolved, "team/proj", &HeaderMap::new())
        .await
        .unwrap();
    assert!(html.contains("First release"));
    assert!(html.contains("view=release&amp;tag=v1.0") || html.contains("view=release&tag=v1.0"));
    // Anonymous viewer: no create-release form.
    assert!(!html.contains(r#"action="/_releases""#));
}

#[tokio::test]
async fn releases_list_hides_a_draft_from_an_anonymous_viewer() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "Unpublished".to_string(),
        notes: String::new(),
        assets: Vec::new(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: true,
    };
    resolved.remote.put_release(&release, None).await.unwrap();

    let html = render_releases_list(&ctx, &resolved, "team/proj", &HeaderMap::new())
        .await
        .unwrap();
    assert!(!html.contains("Unpublished"), "{html}");
    assert!(html.contains("no releases yet"), "{html}");
}

#[tokio::test]
async fn release_detail_404s_for_a_draft_seen_by_an_anonymous_viewer() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "Unpublished".to_string(),
        notes: String::new(),
        assets: Vec::new(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: true,
    };
    resolved.remote.put_release(&release, None).await.unwrap();

    assert!(
        render_release_detail(
            &ctx,
            &resolved,
            "team/proj",
            "v1.0",
            &HeaderMap::new(),
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn release_detail_404s_without_a_release() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    assert!(
        render_release_detail(
            &ctx,
            &resolved,
            "team/proj",
            "v1.0",
            &HeaderMap::new(),
            gitquarry_i18n::Locale::Es
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn release_detail_labels_the_generated_source_assets_but_not_a_regular_one() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    // `handler_create_post` above is what actually generates and
    // uploads these two at release-creation time — here
    // they're just pre-seeded as ordinary `AssetMeta` entries, same as
    // any other upload, to exercise the label-matching this view does
    // on top of them.
    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "First release".to_string(),
        notes: String::new(),
        assets: vec![
            AssetMeta {
                filename: git::browse::archive_filename("team/proj", "v1.0", "tar.gz"),
                size: 100,
                sha256: "a".repeat(64),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                uploaded_by: "alice".to_string(),
                signature: None,
                signature_verified: false,
            },
            AssetMeta {
                filename: git::browse::archive_filename("team/proj", "v1.0", "zip"),
                size: 120,
                sha256: "b".repeat(64),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                uploaded_by: "alice".to_string(),
                signature: None,
                signature_verified: false,
            },
            AssetMeta {
                filename: "notes.pdf".to_string(),
                size: 10,
                sha256: "c".repeat(64),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                uploaded_by: "alice".to_string(),
                signature: None,
                signature_verified: false,
            },
        ],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: false,
    };
    resolved.remote.put_release(&release, None).await.unwrap();
    let mut manifest = Manifest::default();
    manifest.refs.insert("refs/tags/v1.0".to_string(), "a".repeat(40));
    resolved.remote.put_manifest(&manifest, None).await.unwrap();

    let html = render_release_detail(
        &ctx,
        &resolved,
        "team/proj",
        "v1.0",
        &HeaderMap::new(),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(!html.contains("no longer exists on this repo"));
    assert!(html.contains("Source code (tar.gz)"), "{html}");
    assert!(html.contains("Source code (zip)"), "{html}");
    // A regular asset keeps its own filename, not a generic label.
    assert!(html.contains("notes.pdf"), "{html}");
    // Real size/sha256/delete cells, like any other asset — not blank.
    assert!(html.contains("100"), "{html}");
    assert!(html.contains(&"a".repeat(6)), "{html}");
    // Downloads from stored bytes (the real asset route), not the
    // on-demand snapshot route.
    assert!(html.contains("/_raw/release_asset/"), "{html}");
    assert!(!html.contains("/_raw/snapshot/"), "{html}");
}

#[tokio::test]
async fn release_detail_badges_a_signed_asset_by_whether_it_verified() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "First release".to_string(),
        notes: String::new(),
        assets: vec![
            AssetMeta {
                filename: "asset-good.bin".to_string(),
                size: 5,
                sha256: "a".repeat(64),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                uploaded_by: "alice".to_string(),
                signature: Some("-----BEGIN PGP SIGNATURE-----...".to_string()),
                signature_verified: true,
            },
            AssetMeta {
                filename: "asset-bad.bin".to_string(),
                size: 5,
                sha256: "b".repeat(64),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                uploaded_by: "alice".to_string(),
                signature: Some("-----BEGIN PGP SIGNATURE-----...".to_string()),
                signature_verified: false,
            },
            AssetMeta {
                filename: "asset-plain.bin".to_string(),
                size: 5,
                sha256: "c".repeat(64),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                uploaded_by: "alice".to_string(),
                signature: None,
                signature_verified: false,
            },
        ],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: false,
    };
    resolved.remote.put_release(&release, None).await.unwrap();

    let html = render_release_detail(
        &ctx,
        &resolved,
        "team/proj",
        "v1.0",
        &HeaderMap::new(),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let good_row = html.split("<tr>").find(|r| r.contains("asset-good.bin")).unwrap();
    assert!(good_row.contains("signature verified"), "{good_row}");
    let bad_row = html.split("<tr>").find(|r| r.contains("asset-bad.bin")).unwrap();
    assert!(bad_row.contains("signature not verified"), "{bad_row}");
    let plain_row = html.split("<tr>").find(|r| r.contains("asset-plain.bin")).unwrap();
    assert!(!plain_row.contains("signature"), "{plain_row}");
}

#[tokio::test]
async fn release_detail_notes_a_dangling_tag_without_hiding_the_release() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = test_resolved(dir.path());
    let ctx = test_ctx(dir.path());
    let release = ReleaseMeta {
        tag: "v1.0".to_string(),
        title: "First release".to_string(),
        notes: "**bold** notes".to_string(),
        assets: vec![AssetMeta {
            filename: "a.bin".to_string(),
            size: 5,
            sha256: "a".repeat(64),
            uploaded_at: "2024-01-01T00:00:00Z".to_string(),
            uploaded_by: "alice".to_string(),
            signature: None,
            signature_verified: false,
        }],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft: false,
    };
    resolved.remote.put_release(&release, None).await.unwrap();
    // No manifest ref for this tag at all — the "dangling" case.
    resolved.remote.put_manifest(&Manifest::default(), None).await.unwrap();

    let html = render_release_detail(
        &ctx,
        &resolved,
        "team/proj",
        "v1.0",
        &HeaderMap::new(),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("no longer exists on this repo"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("a.bin"));
    // Anonymous viewer: no edit/delete/upload forms.
    assert!(!html.contains(r#"action="/_releases/edit""#));
}

#[test]
fn the_tag_field_suggests_only_tags_without_a_release_yet() {
    let refs = [
        "refs/heads/main".to_string(),
        "refs/tags/v0.2.0".to_string(),
        "refs/tags/v0.1.0".to_string(),
        "refs/tags/v0.3.0".to_string(),
    ];
    let released = ["v0.1.0".to_string()];

    let suggested = taggable_tags(refs.iter(), &released);

    // Branches aren't candidates, a tag that already has a release isn't
    // either (it would only earn the "already has a release" rejection),
    // and what's left comes back sorted rather than in manifest order.
    assert_eq!(suggested, vec!["v0.2.0".to_string(), "v0.3.0".to_string()]);
}

#[test]
fn the_tag_field_suggests_nothing_when_every_tag_is_released() {
    let refs = ["refs/tags/v1.0.0".to_string()];
    assert!(taggable_tags(refs.iter(), &["v1.0.0".to_string()]).is_empty());
    // And a repo with no tags at all leaves the field a plain text input.
    assert!(taggable_tags([].iter(), &[]).is_empty());
}
