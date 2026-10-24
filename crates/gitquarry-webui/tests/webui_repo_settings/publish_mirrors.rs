//! The publish-mirrors section: the entity's form, the dry-run preview,
//! "publish now" and the run log. Repo-owner only, like the external
//! mirrors; only "publish now" pushes.

use crate::support::*;
use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::entities::Permission;
use gitquarry_mirror_external::publish::{self, FilterConfig, FilterMode, MirrorToken, PublishMirror, Publisher};
use gitquarry_test_support::{Work, git};

async fn seed(gq: &Instance) -> PublishMirror {
    let uid = gq.resolved().await.uid;
    let mirror = PublishMirror {
        label: "public".to_string(),
        remote_url: "https://github.com/acme/public.git".to_string(),
        filter: FilterConfig {
            mode: FilterMode::Exclude,
            exclude: vec!["deploy/**".to_string()],
            ..Default::default()
        },
        publish_branches: ["main".to_string()].into(),
        ..Default::default()
    };
    publish::create(&gq.ctx, &uid, mirror).await.unwrap()
}

#[tokio::test]
async fn empty_section_offers_the_allowlist_mode_first() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let page = gq.get(&settings_url("settings_publish"), Some(&root)).await;
    assert_eq!(page.status, StatusCode::OK);
    let html = &page.body;
    assert!(html.contains("no publish mirrors yet"), "{html}");
    assert!(html.contains(r#"value="add_publish_mirror""#), "{html}");
    let allowlist = html.find(r#"value="allowlist""#).unwrap();
    let exclude = html.find(r#"value="exclude""#).unwrap();
    assert!(allowlist < exclude, "{html}");
    for action in html.split(r#"name="conf_action" value=""#).skip(1) {
        let action = action.split('"').next().unwrap();
        assert_eq!(action, "add_publish_mirror", "phase 0 offers no other action: {html}");
    }
}

#[tokio::test]
async fn the_owner_adds_one_and_a_maintainer_cannot() {
    let gq = Instance::new().await;
    gq.user_with_perm("maint", Permission::Maintainer).await;
    let fields = [
        ("conf_action", "add_publish_mirror"),
        ("label", "public"),
        ("remote_url", "https://github.com/acme/public.git"),
        ("publish_mode", "allowlist"),
        ("publish_include", "src/**\nLICENSE\n"),
        ("publish_branches", "main, release/*"),
        (
            "publish_identities",
            "ana@internal.corp ana@users.noreply.example.org Ana Pérez",
        ),
    ];

    let maint = gq.login("maint").await;
    let refused = gq.save(&maint, &fields).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);

    let alice = gq.login("alice").await;
    let saved = gq.save(&alice, &fields).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    let uid = gq.resolved().await.uid;
    let stored = publish::list_with_etags(&gq.ctx, &uid).await.unwrap();
    assert_eq!(stored.len(), 1);
    let mirror = &stored[0].0;
    assert_eq!(mirror.filter.include, ["src/**", "LICENSE"]);
    assert_eq!(mirror.identities[0].name.as_deref(), Some("Ana Pérez"));
    assert!(mirror.publish_branches.contains("release/*"));
}

#[tokio::test]
async fn a_ref_pattern_aimed_at_merge_requests_is_refused_with_a_flash() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let page = gq
        .save(
            &root,
            &[
                ("conf_action", "add_publish_mirror"),
                ("remote_url", "https://github.com/acme/public.git"),
                ("publish_branches", "refs/merge-requests/*"),
            ],
        )
        .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("flash-err"), "{}", page.body);
    let uid = gq.resolved().await.uid;
    assert!(publish::list_with_etags(&gq.ctx, &uid).await.unwrap().is_empty());
}

#[tokio::test]
async fn preview_renders_paths_gates_and_the_public_head_escaped() {
    let gq = Instance::new().await;
    let mirror = seed(&gq).await;
    let work = Work::new().await;
    work.commit(
        &[
            ("LICENSE", "SPDX-License-Identifier: MIT\n"),
            ("src/<b>bold</b>.rs", "\n"),
            ("prod.env", "X=1\n"),
        ],
        "start\n",
    )
    .await;
    std::fs::create_dir_all(work.path().join("deploy")).unwrap();
    git(work.path(), &["mv", "prod.env", "deploy/prod.env"]).await;
    let head = work.commit(&[], "hide the env\n").await;
    let resolved = gq.resolved().await;
    work.push(&resolved, "refs/heads/main").await;
    let manifest_before = resolved.remote.get_manifest().await.unwrap().0.refs;

    let root = gq.login("root").await;
    let page = gq
        .save(
            &root,
            &[
                ("conf_action", "preview_publish_mirror"),
                ("publish_mirror_id", &mirror.id),
            ],
        )
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let html = &page.body;
    assert!(html.contains("nothing was pushed"), "{html}");
    assert!(html.contains(&head), "private head shown: {html}");
    assert!(html.contains("blocked: a publish run would refuse to start"), "{html}");
    assert!(html.contains("blob-set audit"), "{html}");
    assert!(html.contains("deploy/prod.env"), "{html}");
    assert!(html.contains("config: deploy/**: 1"), "{html}");
    assert!(html.contains("src/&lt;b&gt;bold&lt;/b&gt;.rs"), "{html}");
    assert!(!html.contains("<b>bold</b>"), "{html}");

    let manifest_after = gq.resolved().await.remote.get_manifest().await.unwrap().0.refs;
    assert_eq!(manifest_before, manifest_after);
}

#[tokio::test]
async fn preview_without_the_csrf_token_is_refused() {
    let gq = Instance::new().await;
    let mirror = seed(&gq).await;
    let root = gq.login("root").await;
    let forged = Session {
        cookie: root.cookie.clone(),
        csrf: String::new(),
    };
    let page = gq
        .save(
            &forged,
            &[
                ("conf_action", "preview_publish_mirror"),
                ("publish_mirror_id", &mirror.id),
            ],
        )
        .await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST);
}

const TOKEN: &str = "s3cr3t-publish-token";

async fn set_extra_hosts(ctx: &AppContext, hosts: &[&str]) {
    let (mut settings, etag) = gitquarry_core::storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher)
        .await
        .unwrap();
    settings.mirror_extra_hosts = hosts.iter().map(|h| h.to_string()).collect();
    gitquarry_core::storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher)
        .await
        .unwrap();
}

/// A mirror saved while its host was allowed, whose host the instance admin
/// then dropped from the allowlist: "publish now" must stop at the host gate
/// at publish time, without reaching any network.
async fn seed_on_a_host_since_disallowed(gq: &Instance) -> PublishMirror {
    set_extra_hosts(&gq.ctx, &["example.invalid"]).await;
    let uid = gq.resolved().await.uid;
    let mirror = publish::create(
        &gq.ctx,
        &uid,
        PublishMirror {
            label: "public".to_string(),
            remote_url: "https://example.invalid/acme/public.git".to_string(),
            token: MirrorToken::new(TOKEN.to_string()),
            enabled: true,
            publisher: Some(Publisher {
                name: "Acme".to_string(),
                email: "publish@acme.example".to_string(),
            }),
            publish_branches: ["main".to_string()].into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    set_extra_hosts(&gq.ctx, &[]).await;
    mirror
}

#[tokio::test]
async fn publish_now_surfaces_the_error_to_the_click_and_logs_the_run() {
    let gq = Instance::new().await;
    let mirror = seed_on_a_host_since_disallowed(&gq).await;
    let alice = gq.login("alice").await;

    let page = gq
        .save(
            &alice,
            &[("conf_action", "publish_mirror_now"), ("publish_mirror_id", &mirror.id)],
        )
        .await;

    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let html = &page.body;
    assert!(html.contains("flash-err"), "{html}");
    assert!(
        html.contains("example.invalid"),
        "the host gate's reason reaches the click: {html}"
    );
    assert!(html.contains("last runs (1)"), "{html}");
    assert!(html.contains("blocked"), "{html}");
    assert!(html.contains(r#"value="publish_mirror_now""#), "{html}");
    assert!(!html.contains(TOKEN), "the token is never rendered: {html}");
    let log = publish::log::get(&gq.ctx, &gq.resolved().await.uid, &mirror.id)
        .await
        .unwrap();
    assert!(log.last_error.unwrap().contains("example.invalid"));
}

#[tokio::test]
async fn only_the_owner_can_publish_now_and_only_with_the_csrf_token() {
    let gq = Instance::new().await;
    let mirror = seed_on_a_host_since_disallowed(&gq).await;
    gq.user_with_perm("maint", Permission::Maintainer).await;
    let fields = [
        ("conf_action", "publish_mirror_now"),
        ("publish_mirror_id", mirror.id.as_str()),
    ];

    let maint = gq.login("maint").await;
    assert_eq!(gq.save(&maint, &fields).await.status, StatusCode::FORBIDDEN);
    let alice = gq.login("alice").await;
    let forged = Session {
        cookie: alice.cookie.clone(),
        csrf: String::new(),
    };
    assert_eq!(gq.save(&forged, &fields).await.status, StatusCode::BAD_REQUEST);

    let log = publish::log::get(&gq.ctx, &gq.resolved().await.uid, &mirror.id)
        .await
        .unwrap();
    assert!(log.runs.is_empty(), "neither refused click ran anything");
}

#[tokio::test]
async fn an_edit_with_a_blank_token_keeps_the_stored_one() {
    let gq = Instance::new().await;
    let mirror = seed_on_a_host_since_disallowed(&gq).await;
    set_extra_hosts(&gq.ctx, &["example.invalid"]).await;
    let uid = gq.resolved().await.uid;
    let (_, etag) = publish::get(&gq.ctx, &uid, &mirror.id).await.unwrap().unwrap();
    let alice = gq.login("alice").await;

    let page = gq
        .save(
            &alice,
            &[
                ("conf_action", "edit_publish_mirror"),
                ("publish_mirror_id", &mirror.id),
                ("publish_mirror_etag", &etag),
                ("label", "renamed"),
                ("remote_url", "https://example.invalid/acme/public.git"),
                ("token", ""),
                ("publish_run_mode", "snapshot"),
                ("publish_publisher_name", "Acme"),
                ("publish_publisher_email", "publish@acme.example"),
                ("publish_branches", "main"),
            ],
        )
        .await;

    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(!page.body.contains(TOKEN), "{}", page.body);
    let (stored, _) = publish::get(&gq.ctx, &uid, &mirror.id).await.unwrap().unwrap();
    assert_eq!(stored.label, "renamed");
    assert_eq!(stored.token.expose(), TOKEN);
    assert!(!stored.enabled, "an unchecked box turns the sweep off");
}

#[tokio::test]
async fn a_truncated_preview_says_so_and_is_not_a_pass() {
    use gitquarry_mirror_external::publish::preview::{MAX_PREVIEW_COMMITS, MAX_PREVIEW_PATHS};
    use std::io::Write;
    let gq = Instance::new().await;
    let mirror = publish::create(
        &gq.ctx,
        &gq.resolved().await.uid,
        PublishMirror {
            remote_url: "https://github.com/acme/public.git".to_string(),
            publish_branches: ["main".to_string()].into(),
            unmapped: publish::UnmappedPolicy::Passthrough,
            filter: FilterConfig {
                mode: FilterMode::Exclude,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let work = Work::new().await;
    let mut stream = String::new();
    for i in 0..=MAX_PREVIEW_COMMITS {
        let message = format!("c{i}\n");
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter T <t@example.com> {} +0000\ndata {}\n{message}",
            1_700_000_000 + i,
            message.len()
        ));
        let mut files = vec![("file.txt".to_string(), format!("{i}\n"))];
        if i == 0 {
            files.push(("LICENSE".to_string(), "SPDX-License-Identifier: MIT\n".to_string()));
            files.extend((0..MAX_PREVIEW_PATHS).map(|n| (format!("d/f{n:05}.txt"), "x\n".to_string())));
        }
        for (path, body) in files {
            stream.push_str(&format!("M 100644 inline {path}\ndata {}\n{body}\n", body.len()));
        }
    }
    let mut import = std::process::Command::new("git")
        .current_dir(work.path())
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    import.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(import.wait().unwrap().success());
    work.push(&gq.resolved().await, "refs/heads/main").await;

    let root = gq.login("root").await;
    let page = gq
        .save(
            &root,
            &[
                ("conf_action", "preview_publish_mirror"),
                ("publish_mirror_id", &mirror.id),
            ],
        )
        .await;

    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let html = &page.body;
    assert!(html.contains("history truncated"), "{html}");
    assert!(html.contains("inconclusive"), "{html}");
    assert!(!html.contains("every gate passes"), "{html}");
    assert!(html.contains("list truncated"), "{html}");
}
