use crate::support::*;

#[tokio::test]
async fn labels_form_shows_the_saved_value_and_pills() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    gq.save(&root, &[("conf_action", "set_labels"), ("labels", "bug:#d73a4a")])
        .await;
    let html = gq.get(&settings_url("settings_labels"), Some(&root)).await.body;
    assert!(html.contains(r#"value="bug:#d73a4a""#), "{html}");
    assert!(html.contains(r#"value="set_labels""#), "{html}");
    assert!(html.contains("#d73a4a"), "{html}");
}
