use crate::support::*;

#[tokio::test]
async fn topics_form_shows_the_saved_value() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    gq.save(&root, &[("conf_action", "set_topics"), ("topics", "rust")])
        .await;
    let html = gq.get(&settings_url("settings_topics"), Some(&root)).await.body;
    assert!(html.contains(r#"value="rust""#), "{html}");
    assert!(html.contains(r#"value="set_topics""#), "{html}");
}
