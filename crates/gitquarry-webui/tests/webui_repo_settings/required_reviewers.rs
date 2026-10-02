use crate::support::*;

#[tokio::test]
async fn required_reviewers_form_is_empty_until_reviewers_are_saved() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_reviewers"), Some(&root)).await.body;
    assert!(html.contains(r#"name="required_reviewers" value="""#), "{html}");

    gq.save(
        &root,
        &[
            ("conf_action", "set_required_reviewers"),
            ("required_reviewers", "alice"),
        ],
    )
    .await;
    let html = gq.get(&settings_url("settings_reviewers"), Some(&root)).await.body;
    assert!(html.contains(r#"value="alice""#), "{html}");
    assert!(html.contains(r#"value="set_required_reviewers""#), "{html}");
}
