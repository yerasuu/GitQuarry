use crate::support::*;

#[tokio::test]
async fn protected_branches_form_is_absent_without_any_branch() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq
        .get(&settings_url("settings_protected_branches"), Some(&root))
        .await
        .body;
    assert!(!html.contains(r#"value="set_protected_branches""#), "{html}");
}

#[tokio::test]
async fn protected_branches_form_shows_the_saved_value_and_available_branches() {
    let gq = Instance::new().await;
    gq.set_refs(&["refs/heads/main", "refs/heads/dev"], None).await;
    let root = gq.login("root").await;
    gq.save(
        &root,
        &[("conf_action", "set_protected_branches"), ("protected", "main")],
    )
    .await;
    let html = gq
        .get(&settings_url("settings_protected_branches"), Some(&root))
        .await
        .body;
    assert!(html.contains(r#"name="protected" value="main""#), "{html}");
    assert!(html.contains("main, dev") || html.contains("dev, main"), "{html}");
    assert!(html.contains(r#"value="set_protected_branches""#), "{html}");
}
