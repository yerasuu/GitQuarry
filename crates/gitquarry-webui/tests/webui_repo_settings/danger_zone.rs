use crate::support::*;
use axum::http::StatusCode;

#[tokio::test]
async fn delete_repo_form_placeholders_the_full_name() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_danger_zone"), Some(&root)).await.body;
    assert!(html.contains(r#"placeholder="alice/proj""#), "{html}");
    assert!(html.contains(r#"value="delete_repo""#), "{html}");
}

#[tokio::test]
async fn a_delete_without_the_matching_name_is_refused() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let page = gq
        .save(&root, &[("conf_action", "delete_repo"), ("confirm_name", "alice/pro")])
        .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(gq.config().await.deleted_at.is_none());
}

/// A soft-deleted repo offers restore and a forced purge instead of
/// another delete.
#[tokio::test]
async fn a_deleted_repo_shows_restore_and_purge_instead() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let deleted = gq
        .save(&root, &[("conf_action", "delete_repo"), ("confirm_name", REPO)])
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    assert_eq!(deleted.location(), "/");

    let html = gq.get(&settings_url("settings_danger_zone"), Some(&root)).await.body;
    assert!(html.contains(r#"value="restore_repo""#), "{html}");
    assert!(!html.contains(r#"value="delete_repo""#), "{html}");
    assert!(html.contains(r#"value="purge_repo""#), "{html}");
    assert!(html.contains(r#"placeholder="alice/proj""#), "{html}");
}
