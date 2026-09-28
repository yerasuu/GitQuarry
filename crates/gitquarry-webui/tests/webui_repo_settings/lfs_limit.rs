use crate::support::*;

#[tokio::test]
async fn lfs_limit_form_is_blank_until_a_limit_is_saved() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_lfs"), Some(&root)).await.body;
    assert!(html.contains(r#"name="lfs_max_blob_size" value="""#), "{html}");

    gq.save(
        &root,
        &[("conf_action", "set_lfs_max_blob_size"), ("lfs_max_blob_size", "50M")],
    )
    .await;
    let html = gq.get(&settings_url("settings_lfs"), Some(&root)).await.body;
    assert!(html.contains(r#"value="50M""#), "{html}");
    assert!(html.contains(r#"value="set_lfs_max_blob_size""#), "{html}");
}
