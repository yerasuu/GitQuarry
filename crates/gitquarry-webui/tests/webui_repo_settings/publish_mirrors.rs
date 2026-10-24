//! The publish-mirrors section: the entity's form, and the dry-run preview
//! that is the only thing phase 0 can do with one. Repo-owner only, like
//! the external mirrors, and nothing in it pushes.

use crate::support::*;
use axum::http::StatusCode;
use gitquarry_core::auth::entities::Permission;
use gitquarry_mirror_external::publish::{self, FilterConfig, FilterMode, PublishMirror};
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
