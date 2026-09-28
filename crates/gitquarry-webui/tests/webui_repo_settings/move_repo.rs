use crate::support::*;
use axum::http::StatusCode;

#[tokio::test]
async fn move_repo_form_lists_users_and_groups_as_namespace_options() {
    let gq = Instance::new().await;
    let (root_user, _) = gitquarry_core::auth::access::get_user(gq.ctx.backend().as_ref(), &gq.ctx.cipher, "root")
        .await
        .unwrap();
    gitquarry_core::auth::domain::group::create(
        &gq.ctx,
        "acme",
        None,
        root_user.unwrap().uid,
        None,
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap()
    .unwrap();
    let root = gq.login("root").await;
    let html = gq.get(&settings_url("settings_move"), Some(&root)).await.body;
    assert!(html.contains(r#"<option value="alice">alice</option>"#), "{html}");
    assert!(html.contains(r#"<option value="acme">acme</option>"#), "{html}");
    assert!(html.contains(r#"placeholder="alice/proj""#), "{html}");
    assert!(html.contains(r#"value="move_repo""#), "{html}");
}

/// Typing the full name is the only friction before every existing clone
/// URL breaks.
#[tokio::test]
async fn a_move_without_the_matching_name_is_refused() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let page = gq
        .save(
            &root,
            &[
                ("conf_action", "move_repo"),
                ("new_namespace", "root"),
                ("new_local_name", "proj"),
                ("confirm_name", "alice/other"),
            ],
        )
        .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(gq.ctx.resolve_repo(REPO).await.unwrap().is_some());
    assert!(gq.ctx.resolve_repo("root/proj").await.unwrap().is_none());
}
