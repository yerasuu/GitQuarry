//! The external-mirrors section. A mirror's token is a live credential for
//! a third-party forge: it's shown once, in the response to the save that
//! set it, and never rendered back out of storage afterwards.

use crate::support::*;
use axum::http::StatusCode;
use gitquarry_mirror_external::access::{
    self, ExternalMirror, ExternalMirrorProvider, ExternalPrLink, RefSyncStatus, SyncDirection,
};

fn sample_mirror() -> ExternalMirror {
    ExternalMirror {
        remote_url: "https://github.com/acme/demo.git".to_string(),
        token: "ghp_secret".to_string(),
        provider: ExternalMirrorProvider::GitHub,
        direction: SyncDirection::Bidirectional,
        enabled: true,
        label: "backup".to_string(),
        ..Default::default()
    }
}

async fn seed(gq: &Instance, mirror: ExternalMirror) -> ExternalMirror {
    let uid = gq.resolved().await.uid;
    gitquarry_mirror_external::domain::create(&gq.ctx, &uid, mirror)
        .await
        .unwrap()
}

async fn mirrors_page(gq: &Instance) -> String {
    let root = gq.login("root").await;
    let page = gq.get(&settings_url("settings_mirrors"), Some(&root)).await;
    assert_eq!(page.status, StatusCode::OK);
    page.body
}

#[tokio::test]
async fn empty_list_shows_hint_and_an_empty_add_form() {
    let gq = Instance::new().await;
    let html = mirrors_page(&gq).await;
    assert!(html.contains("no mirrors configured yet"), "{html}");
    assert!(html.contains(r#"value="add_mirror""#), "{html}");
    assert!(html.contains(r#"name="branch_patterns" value="""#), "{html}");
}

#[tokio::test]
async fn lists_a_mirror_row_with_its_actions_and_never_its_token() {
    let gq = Instance::new().await;
    seed(&gq, sample_mirror()).await;
    let html = mirrors_page(&gq).await;
    assert!(html.contains("backup"), "{html}");
    assert!(html.contains("GitHub"), "{html}");
    assert!(html.contains(r#"value="sync_mirror_now""#), "{html}");
    assert!(html.contains(r#"value="delete_mirror""#), "{html}");
    assert!(html.contains(r#"value="edit_mirror""#), "{html}");
    assert!(!html.contains("ghp_secret"), "{html}");
}

#[tokio::test]
async fn a_new_token_is_shown_once_in_the_save_response_only() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let saved = gq
        .save(
            &root,
            &[
                ("conf_action", "add_mirror"),
                ("remote_url", "https://github.com/acme/demo.git"),
                ("token", "ghp_brand_new"),
                ("label", "backup"),
                ("mirror_enabled", "1"),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    assert!(saved.body.contains("<code>ghp_brand_new</code>"), "{}", saved.body);
    assert!(saved.body.contains("backup"), "{}", saved.body);

    let reloaded = gq.get(&settings_url("settings_mirrors"), Some(&root)).await.body;
    assert!(reloaded.contains("backup"), "{reloaded}");
    assert!(!reloaded.contains("ghp_brand_new"), "{reloaded}");
}

/// A blank token on an edit keeps the stored one, so there's nothing new
/// to show.
#[tokio::test]
async fn an_edit_that_keeps_the_token_shows_none() {
    let gq = Instance::new().await;
    let mirror = seed(&gq, sample_mirror()).await;
    let uid = gq.resolved().await.uid;
    let (_, etag) = gitquarry_mirror_external::domain::get(&gq.ctx, &uid, &mirror.id)
        .await
        .unwrap()
        .unwrap();
    let root = gq.login("root").await;
    let saved = gq
        .save(
            &root,
            &[
                ("conf_action", "edit_mirror"),
                ("mirror_id", &mirror.id),
                ("mirror_etag", &etag),
                ("remote_url", "https://github.com/acme/demo.git"),
                ("token", ""),
                ("label", "renamed"),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    assert!(saved.body.contains("renamed"), "{}", saved.body);
    assert!(!saved.body.contains("ghp_secret"), "{}", saved.body);
    let (stored, _) = gitquarry_mirror_external::domain::get(&gq.ctx, &uid, &mirror.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.token, "ghp_secret");
}

#[tokio::test]
async fn edit_form_prefills_branch_patterns_as_comma_separated() {
    let gq = Instance::new().await;
    seed(
        &gq,
        ExternalMirror {
            branch_patterns: std::collections::BTreeSet::from(["main".to_string(), "release/*".to_string()]),
            ..sample_mirror()
        },
    )
    .await;
    let html = mirrors_page(&gq).await;
    assert!(
        html.contains(r#"name="branch_patterns" value="main, release/*""#),
        "{html}"
    );
}

/// The edit's CAS guard has to be what the browser loaded, or a concurrent
/// edit landing in between would go unnoticed.
#[tokio::test]
async fn edit_form_carries_the_mirrors_real_etag_as_its_cas_guard() {
    let gq = Instance::new().await;
    let mirror = seed(&gq, sample_mirror()).await;
    let uid = gq.resolved().await.uid;
    let (_, etag) = gitquarry_mirror_external::domain::get(&gq.ctx, &uid, &mirror.id)
        .await
        .unwrap()
        .unwrap();
    let html = mirrors_page(&gq).await;
    assert!(
        html.contains(&format!(r#"name="mirror_etag" value="{etag}""#)),
        "{html}"
    );
}

#[tokio::test]
async fn ref_status_pills_render_each_ref_with_its_own_status() {
    let gq = Instance::new().await;
    let mirror = seed(&gq, sample_mirror()).await;
    let uid = gq.resolved().await.uid;
    for (ref_name, status) in [
        ("refs/heads/main", RefSyncStatus::InSync),
        ("refs/heads/dev", RefSyncStatus::Diverged),
    ] {
        access::set_ref_status(
            gq.ctx.backend().as_ref(),
            &gq.ctx.cipher,
            &uid,
            &mirror.id,
            ref_name,
            status,
        )
        .await
        .unwrap();
    }
    let html = mirrors_page(&gq).await;
    assert!(html.contains("refs/heads/main"), "{html}");
    assert!(html.contains("in sync"), "{html}");
    assert!(html.contains("refs/heads/dev"), "{html}");
    assert!(html.contains("diverged"), "{html}");
}

#[tokio::test]
async fn a_whole_mirror_failure_shows_with_its_message_in_the_tooltip() {
    let gq = Instance::new().await;
    let mirror = seed(&gq, sample_mirror()).await;
    let uid = gq.resolved().await.uid;
    access::set_mirror_error(
        gq.ctx.backend().as_ref(),
        &gq.ctx.cipher,
        &uid,
        &mirror.id,
        Some("couldn't reach the remote".to_string()),
    )
    .await
    .unwrap();
    let html = mirrors_page(&gq).await;
    assert!(html.contains("sync failed"), "{html}");
    assert!(html.contains(r#"title="couldn't reach the remote""#), "{html}");
    // The "nothing synced yet" dash is for a mirror with no status at all,
    // not one that just failed outright.
    assert!(!html.contains("<em>—</em>"), "{html}");
}

#[tokio::test]
async fn linked_prs_show_only_for_a_mirror_with_mr_sync_on() {
    let gq = Instance::new().await;
    let synced = seed(
        &gq,
        ExternalMirror {
            sync_merge_requests: true,
            ..sample_mirror()
        },
    )
    .await;
    let uid = gq.resolved().await.uid;
    let link = ExternalPrLink {
        local_number: 3,
        external_number: 7,
        remote_updated_cursor: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    access::put_mr_link(gq.ctx.backend().as_ref(), &gq.ctx.cipher, &uid, &synced.id, &link, None)
        .await
        .unwrap();
    let html = mirrors_page(&gq).await;
    assert!(html.contains("linked merge requests"), "{html}");
    assert!(html.contains("#3"), "{html}");
    assert!(html.contains("#7"), "{html}");

    let gq = Instance::new().await;
    seed(&gq, sample_mirror()).await;
    let html = mirrors_page(&gq).await;
    assert!(!html.contains("linked merge requests"), "{html}");
}
