//! A repo's landing pages: the clone URLs, the language bar and the
//! license the sidebar names, all read off real pushed content.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

fn test_ctx(root: &Path, ssh_bind: &str) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: ssh_bind.to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
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

async fn git(dir: &Path, args: &[&str]) -> String {
    // Signing is forced off so a developer's global `commit.gpgsign` can't
    // turn every commit here into a pinentry prompt.
    let out = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2024-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2024-01-01T00:00:00Z")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// What a `git push` of every ref in `work` leaves in the bucket: its
/// objects as one uploaded pack, its refs in the manifest.
async fn push_all(ctx: &AppContext, name: &str, work: &Path) {
    let resolved = ctx.resolve_repo(name).await.unwrap().unwrap();
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_all_objects(&work.join(".git"), packs.path())
        .await
        .unwrap()
        .unwrap();
    resolved
        .remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let refs = git(work, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .await
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(name, sha)| (name.to_string(), sha.to_string()))
        .collect();
    let (_, etag) = resolved.remote.get_manifest().await.unwrap();
    resolved
        .remote
        .put_manifest(
            &Manifest {
                refs,
                head: Some("refs/heads/main".to_string()),
            },
            etag.as_deref(),
        )
        .await
        .unwrap();
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let mut request = Request::builder()
        .uri(uri)
        .header(header::HOST, "git.example.com:8080")
        .body(Body::empty())
        .unwrap();
    // `run_server` serves with `into_make_service_with_connect_info`; a bare
    // `oneshot` doesn't, and the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// `name`, public, holding one commit of `files` on `main` — `None` leaves
/// it empty, never pushed to.
async fn repo_with(ctx: &AppContext, name: &str, files: Option<&[(&str, &str)]>) {
    gitquarry_core::domain::repo::create(ctx, name, None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let Some(files) = files else { return };
    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    git(w, &["init", "--quiet", "--initial-branch=main"]).await;
    for (path, content) in files {
        std::fs::write(w.join(path), content).unwrap();
    }
    git(w, &["add", "."]).await;
    git(w, &["commit", "--quiet", "-m", "init"]).await;
    push_all(ctx, name, w).await;
}

async fn page(app: &Router, uri: &str) -> String {
    let (status, body) = get(app, uri).await;
    assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
    body
}

#[tokio::test]
async fn an_empty_repo_shows_both_clone_urls_without_a_saved_remote_setting() {
    // No `.conf/global.json` written at all in this fresh bucket: the ssh://
    // row must still render, from `ctx.ssh_bind`, not from a remote setting
    // most deployments never save.
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), "0.0.0.0:9999"));
    repo_with(&ctx, "acme/web", None).await;
    let html = page(&gitquarry_webui::webui::router(ctx), "/acme/web").await;
    assert!(html.contains("http://git.example.com:8080/acme/web"), "{html}");
    assert!(html.contains("ssh://git.example.com:9999/acme/web"), "{html}");
}

#[tokio::test]
async fn the_summary_tab_shows_both_clone_urls() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), "127.0.0.1:2222"));
    repo_with(&ctx, "acme/web", Some(&[("notes.txt", "hi\n")])).await;
    let html = page(&gitquarry_webui::webui::router(ctx), "/acme/web?view=summary").await;
    assert!(html.contains("http://git.example.com:8080/acme/web"), "{html}");
    assert!(html.contains("ssh://git.example.com:2222/acme/web"), "{html}");
}

#[tokio::test]
async fn language_bar_shows_each_language_with_its_percentage() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), "127.0.0.1:2222"));
    repo_with(
        &ctx,
        "acme/web",
        Some(&[("main.rs", &"r".repeat(75)), ("style.css", &"c".repeat(25))]),
    )
    .await;
    let html = page(&gitquarry_webui::webui::router(ctx), "/acme/web").await;
    // The share is a class, not a style attribute -- see webui_csp.rs.
    assert!(html.contains(r#"class="w-75 lang-c-rust""#), "{html}");
    assert!(html.contains("Rust 75.0%"), "{html}");
    assert!(html.contains("CSS 25.0%"), "{html}");
}

#[tokio::test]
async fn language_bar_empty_without_any_language() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), "127.0.0.1:2222"));
    repo_with(
        &ctx,
        "acme/web",
        Some(&[("notes.txt", "no extension a language maps to\n")]),
    )
    .await;
    let html = page(&gitquarry_webui::webui::router(ctx), "/acme/web").await;
    assert!(html.contains("(no languages detected)"), "{html}");
    // `lang-bar` alone also appears in the inlined stylesheet.
    assert!(!html.contains(r#"class="lang-bar""#), "{html}");
}

#[tokio::test]
async fn a_language_linguist_does_not_know_still_gets_a_stable_palette_class() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), "127.0.0.1:2222"));
    for (name, language) in [
        ("acme/one", "Brainfuck"),
        ("acme/two", "Brainfuck"),
        ("acme/three", "Rust"),
    ] {
        let resolved = gitquarry_core::domain::repo::create(&ctx, name, None, None, None, Visibility::Public)
            .await
            .unwrap()
            .unwrap();
        let backend = ctx.backend();
        let uid = resolved.uid.as_str();
        let (mut config, etag) = gitquarry_core::storage::settings::get_repo_config(backend.as_ref(), uid, &ctx.cipher)
            .await
            .unwrap();
        config.primary_language = Some(language.to_string());
        gitquarry_core::storage::settings::put_repo_config(
            backend.as_ref(),
            uid,
            &config,
            etag.as_deref(),
            &ctx.cipher,
        )
        .await
        .unwrap();
    }
    let html = page(&gitquarry_webui::webui::router(ctx), "/").await;

    let swatches: Vec<&str> = html
        .split(r#"class="lang-swatch "#)
        .skip(1)
        .map(|rest| &rest[..rest.find('"').unwrap()])
        .collect();
    assert!(swatches.contains(&"lang-c-rust"), "{swatches:?}");
    let fallback: Vec<&str> = swatches.iter().copied().filter(|c| *c != "lang-c-rust").collect();
    // Two repos, each rendered as a card and a compact row: one class for
    // all four, and one of the palette hues `style/utilities.css` defines.
    assert_eq!(fallback.len(), 4, "{swatches:?}");
    assert!(fallback.iter().all(|c| *c == fallback[0]), "{swatches:?}");
    assert!(fallback[0].starts_with("lang-c-h"), "{swatches:?}");
}

#[tokio::test]
async fn the_sidebar_names_the_license_the_root_file_holds() {
    let bsd3 = "Redistribution and use in source and binary forms... neither the name of the copyright holder... \
                may be used to endorse or promote products";
    let cases = [
        (
            "MIT License\n\nPermission is hereby granted, free of charge, to any person...",
            "MIT",
        ),
        (
            "                                 Apache License\n                           Version 2.0, January 2004",
            "Apache-2.0",
        ),
        // Checked ahead of the generic "GNU" family, so the version wins.
        (
            "GNU GENERAL PUBLIC LICENSE\n                       Version 3, 29 June 2007",
            "GPL-3.0",
        ),
        (
            "Redistribution and use in source and binary forms, with or without modification, are permitted",
            "BSD-2-Clause",
        ),
        (bsd3, "BSD-3-Clause"),
        // Unrecognized text gets the generic label, never a wrong guess.
        ("just some random file content", "license"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path(), "127.0.0.1:2222"));
    for (i, (text, _)) in cases.iter().enumerate() {
        repo_with(&ctx, &format!("acme/l{i}"), Some(&[("LICENSE", text)])).await;
    }
    let app = gitquarry_webui::webui::router(ctx);
    for (i, (_, kind)) in cases.iter().enumerate() {
        let html = page(&app, &format!("/acme/l{i}")).await;
        let link = format!(r#"href="/acme/l{i}/blob/main/LICENSE">{kind}</a>"#);
        assert!(html.contains(&link), "expected {link}: {html}");
    }
}
