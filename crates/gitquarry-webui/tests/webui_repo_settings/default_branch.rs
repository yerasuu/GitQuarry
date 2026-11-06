use crate::support::*;

#[tokio::test]
async fn default_branch_form_marks_the_current_head_selected() {
    let gq = Instance::new().await;
    gq.set_refs(
        &["refs/heads/main", "refs/heads/dev", "refs/tags/v1"],
        Some("refs/heads/dev"),
    )
    .await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_default_branch"), Some(&root)).await.body;
    assert!(html.contains(r#"<option value="dev" selected>dev</option>"#), "{html}");
    assert!(html.contains(r#"<option value="main">main</option>"#), "{html}");
    // A tag isn't a branch.
    assert!(!html.contains(r#"<option value="v1""#), "{html}");
}

#[tokio::test]
async fn default_branch_form_is_absent_without_any_branch() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_default_branch"), Some(&root)).await.body;
    assert!(!html.contains(r#"value="set_default_branch""#), "{html}");
}

#[tokio::test]
async fn saving_the_default_branch_moves_head() {
    let gq = Instance::new().await;
    gq.set_refs(&["refs/heads/main", "refs/heads/dev"], Some("refs/heads/dev"))
        .await;
    let root = gq.login("root").await;
    gq.save(&root, &[("conf_action", "set_default_branch"), ("branch", "main")])
        .await;
    let (manifest, _) = gq.resolved().await.remote.get_manifest().await.unwrap();
    assert_eq!(manifest.head.as_deref(), Some("refs/heads/main"));
}
