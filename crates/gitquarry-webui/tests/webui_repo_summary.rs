use axum::http::HeaderMap;
use gitquarry_core::app::AppContext;
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_webui::webui::repo::summary::*;

fn test_ctx(root: &std::path::Path, ssh_bind: &str) -> AppContext {
    let backend: std::sync::Arc<dyn gitquarry_core::storage::Backend> =
        std::sync::Arc::new(LocalBackend::new(&LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: LocalEngine::Files,
        }));
    let cipher = std::sync::Arc::new(Cipher::derive("test passphrase", &[5u8; SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: ssh_bind.to_string(),
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
        events: Default::default(),
        mirror_sync: None,
    }
}

#[tokio::test]
async fn clone_urls_always_show_both_http_and_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), "0.0.0.0:9999");
    let html = render_clone_urls(&ctx, "acme/web", &HeaderMap::new())
        .await
        .unwrap()
        .into_string();
    assert!(html.contains("http://"));
    assert!(html.contains("ssh://"));
    assert!(html.contains(":9999/acme/web"));
}

#[tokio::test]
async fn clone_urls_show_ssh_even_without_a_saved_remote_setting() {
    // No `.conf/global.json` written at all in this fresh backend --
    // the ssh:// row must still render, from `ctx.ssh_bind`, not from a
    // remote setting most deployments never save.
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path(), "127.0.0.1:2222");
    let html = render_clone_urls(&ctx, "acme/web", &HeaderMap::new())
        .await
        .unwrap()
        .into_string();
    assert!(html.contains("ssh://"));
    assert!(html.contains(":2222/acme/web"));
}

#[test]
fn language_color_is_deterministic_and_varies_by_name() {
    assert_eq!(language_color("Rust"), language_color("Rust"));
    assert_ne!(language_color("Rust"), language_color("Python"));
}

#[test]
fn language_bar_empty_without_any_language() {
    let html = render_language_bar(&[], gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("no languages detected"));
    assert!(!html.contains("lang-bar"));
}

#[test]
fn license_kind_detects_mit() {
    let text = "MIT License\n\nPermission is hereby granted, free of charge, to any person...";
    assert_eq!(license_kind_from_text(text), Some("MIT"));
}

#[test]
fn license_kind_detects_apache_2() {
    let text = "                                 Apache License\n                           Version 2.0, January 2004";
    assert_eq!(license_kind_from_text(text), Some("Apache-2.0"));
}

#[test]
fn license_kind_detects_gpl_3_over_generic_gnu() {
    let text = "GNU GENERAL PUBLIC LICENSE\n                       Version 3, 29 June 2007";
    assert_eq!(license_kind_from_text(text), Some("GPL-3.0"));
}

#[test]
fn license_kind_distinguishes_bsd_2_and_3_clause() {
    let bsd2 = "Redistribution and use in source and binary forms, with or without modification, are permitted";
    let bsd3 = "Redistribution and use in source and binary forms... neither the name of the copyright holder... may be used to endorse or promote products";
    assert_eq!(license_kind_from_text(bsd2), Some("BSD-2-Clause"));
    assert_eq!(license_kind_from_text(bsd3), Some("BSD-3-Clause"));
}

#[test]
fn license_kind_none_for_unrecognized_text() {
    assert_eq!(license_kind_from_text("just some random file content"), None);
}

#[test]
fn language_bar_shows_each_language_with_its_percentage() {
    let languages = vec![("Rust".to_string(), 75u64), ("CSS".to_string(), 25u64)];
    let html = render_language_bar(&languages, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains("width:75.00%"), "{html}");
    assert!(html.contains("Rust 75.0%"), "{html}");
    assert!(html.contains("CSS 25.0%"), "{html}");
}
