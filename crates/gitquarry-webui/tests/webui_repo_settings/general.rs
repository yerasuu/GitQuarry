//! The general section: description, visibility, archived, template flag.

use crate::support::*;
use axum::http::StatusCode;

/// Regression: `owner` used to be a free `<select>` any admin could point
/// at an unrelated user/group, independent of the repo's actual namespace.
/// `save_conf` now always re-derives it from the namespace, so there's
/// nothing left to submit here — and a hand-crafted `owner` field is ignored.
#[tokio::test]
async fn owner_is_shown_read_only_and_a_submitted_owner_is_ignored() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings"), Some(&root)).await.body;
    assert!(!html.contains(r#"<select name="owner""#), "{html}");
    assert!(html.contains(">alice "), "{html}");
    assert!(html.contains("move the repo below"), "{html}");

    let before = gq.config().await.owner;
    let saved = gq.save(&root, &[("owner", "root"), ("description", "d")]).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    let after = gq.config().await;
    assert_eq!(after.description.as_deref(), Some("d"));
    assert_eq!(after.owner, before);
}

#[tokio::test]
async fn is_template_checkbox_is_unchecked_until_saved_checked() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings"), Some(&root)).await.body;
    assert!(
        html.contains(r#"input type="checkbox" name="is_template" value="1">"#),
        "{html}"
    );

    gq.save(&root, &[("is_template", "1")]).await;
    let html = gq.get(&settings_url("settings"), Some(&root)).await.body;
    assert!(
        html.contains(r#"input type="checkbox" name="is_template" value="1" checked"#),
        "{html}"
    );
}

/// The description is typed by whoever manages the repo and shown to
/// every admin who opens this page.
#[tokio::test]
async fn a_stored_description_is_escaped_back_into_the_form() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    gq.save(&root, &[("description", r#""><script>alert(1)</script>"#)])
        .await;
    let html = gq.get(&settings_url("settings"), Some(&root)).await.body;
    assert!(!html.contains("<script>alert(1)</script>"), "{html}");
    assert!(
        html.contains(r#"name="description" value="&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;""#),
        "{html}"
    );
}
