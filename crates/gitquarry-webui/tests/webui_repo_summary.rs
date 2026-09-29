//! A repo's landing pages: the clone URLs, the language bar and the
//! license the sidebar names, all read off real pushed content.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Work, send};
use std::path::Path;
use std::sync::Arc;

fn test_ctx(root: &Path, ssh_bind: &str) -> AppContext {
    AppContext {
        ssh_bind: ssh_bind.to_string(),
        ..gitquarry_test_support::ctx(root)
    }
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let request = Request::get(uri)
        .header(header::HOST, "git.example.com:8080")
        .body(Body::empty())
        .unwrap();
    let page = send(app, request).await;
    (page.status, page.body)
}

/// `name`, public, holding one commit of `files` on `main` — `None` leaves
/// it empty, never pushed to.
async fn repo_with(ctx: &AppContext, name: &str, files: Option<&[(&str, &str)]>) {
    let resolved = gitquarry_core::domain::repo::create(ctx, name, None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let Some(files) = files else { return };
    let work = Work::new().await;
    work.commit(files, "init").await;
    work.push(&resolved, "refs/heads/main").await;
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
