//! The six `/_settings/<section>` pages: their shared sidebar shell, the
//! async target each form swaps, and the admin gate in front of all of them.

use crate::support::*;
use axum::http::StatusCode;

const SECTIONS: [(&str, &str, &str); 6] = [
    ("footer", "/_settings/footer", "footer"),
    ("network", "/_settings/network", "network"),
    ("mirrors-cache", "/_settings/mirrors-cache", "mirrors cache"),
    ("external-mirrors", "/_settings/external-mirrors", "external mirrors"),
    ("oidc", "/_settings/oidc", "OIDC login"),
    ("backup", "/_settings/backup", "backup"),
];

/// Each section wraps a `#<section>-body` div, which is what its form's
/// `data-async-target` points at (the flash banner sits outside the form,
/// so the default "target is the form itself" wouldn't refresh it).
/// Without either, a submit silently goes back to a full-page POST and
/// brings back the "confirm resubmission" dialog on F5.
#[tokio::test]
async fn each_section_wraps_the_flash_and_form_in_one_async_target() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    for (id, action, _) in SECTIONS {
        let page = gq.get(action, Some(&root)).await;
        assert_eq!(page.status, StatusCode::OK, "{id}");
        let html = &page.body;
        assert!(html.contains(&format!(r#"<div id="{id}-body">"#)), "{id}: {html}");
        // `oidc` alone carries `data-confirm`: a bad value there can break
        // login for everyone, unlike the other sections.
        let form = if id == "oidc" {
            format!(
                r##"<form method="post" action="{action}" data-async data-async-target="#{id}-body" data-confirm="save OIDC config? a bad value can break login for everyone until fixed directly in the bucket.">"##
            )
        } else {
            format!(r##"<form method="post" action="{action}" data-async data-async-target="#{id}-body">"##)
        };
        assert!(html.contains(&form), "{id}: missing {form:?} in {html}");
    }
}

/// Regression: every `/_settings/<section>` page is wrapped in the same
/// `.settings-layout`/`.settings-sidebar`/`.settings-content` shell the
/// account and repo settings pages use.
#[tokio::test]
async fn settings_page_uses_the_standard_sidebar_layout() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get("/_settings/network", Some(&root)).await.body;
    assert!(html.contains(r#"<div class="settings-layout">"#), "{html}");
    assert!(html.contains(r#"<nav class="settings-sidebar""#), "{html}");
    assert!(html.contains(r#"<div class="settings-content">"#), "{html}");
    for (id, href, label) in SECTIONS {
        assert!(
            html.contains(&format!(r#"<a href="{href}">{label}</a>"#))
                || html.contains(&format!(
                    r#"<a class="active" aria-current="page" href="{href}">{label}</a>"#
                )),
            "missing sidebar link to {id:?} in {html}"
        );
    }
}

/// Without this an admin has no visual cue for which page they're on
/// beyond the URL bar.
#[tokio::test]
async fn active_section_is_marked_in_the_sidebar() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    let html = gq.get("/_settings/oidc", Some(&root)).await.body;
    assert!(
        html.contains(r#"<a class="active" aria-current="page" href="/_settings/oidc">OIDC login</a>"#),
        "{html}"
    );
}

#[tokio::test]
async fn an_anonymous_visit_is_sent_to_login_and_back() {
    let gq = Instance::new().await;
    for (id, action, _) in SECTIONS {
        let page = gq.get(action, None).await;
        assert_eq!(page.status, StatusCode::SEE_OTHER, "{id}");
        assert_eq!(page.location(), format!("/_login?return_to={action}"), "{id}");
    }
}

#[tokio::test]
async fn a_non_admin_is_refused_every_section() {
    let gq = Instance::new().await;
    let alice = gq.login("alice").await;
    for (id, action, _) in SECTIONS {
        assert_eq!(gq.get(action, Some(&alice)).await.status, StatusCode::FORBIDDEN, "{id}");
        let post = gq.post(action, &alice, &[("source_link", "alice/x")]).await;
        assert_eq!(post.status, StatusCode::FORBIDDEN, "{id}");
    }
}

#[tokio::test]
async fn a_save_without_the_sessions_csrf_token_changes_nothing() {
    let gq = Instance::new().await;
    let root = gq.login("root").await;
    for csrf in ["", "not-the-token"] {
        let fields = [("csrf", csrf), ("source_link", "root/site")];
        let page = gq
            .send("POST", "/_settings/footer", Some(&root.cookie), Some(&fields))
            .await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "csrf {csrf:?}");
    }
    let (settings, _) = gitquarry_core::storage::settings::get_settings(gq.ctx.backend().as_ref(), &gq.ctx.cipher)
        .await
        .unwrap();
    assert!(settings.source_link.is_none());
}
