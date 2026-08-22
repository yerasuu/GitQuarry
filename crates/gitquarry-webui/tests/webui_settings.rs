use gitquarry_core::storage::entities::ServerSettings;
use gitquarry_webui::settings::web::settings_body;

/// The `#settings-body` wrapper is what `data-async-target` on the form
/// points at (the flash banner sits outside the form, so the default
/// "target is the form itself" wouldn't refresh it) — regression coverage
/// for a submit that silently goes back to a full-page POST (and brings
/// back the "confirm resubmission" dialog on F5) if either the id or the
/// form's own attributes go missing.
#[test]
fn settings_body_wraps_the_flash_and_form_in_one_async_target() {
    let settings = ServerSettings::default();
    let html = settings_body(&settings, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.starts_with(r#"<div id="settings-body">"#), "{html}");
    assert!(
        html.contains(r##"<form method="post" action="/_settings" data-async data-async-target="#settings-body">"##),
        "{html}"
    );
}

/// Regression: `/_settings` uses the same `.settings-layout`/
/// `.settings-sidebar`/`.settings-content` shell `account_settings_page`/
/// `repo_settings::render_settings_section` use — the standard layout for
/// every config page in the app now. Unlike those two, the sidebar links
/// are in-page anchors (`#settings-network`, ...) into one still-single
/// form rather than separate `?view=` pages: `/_settings` submits
/// everything in one `PUT`, so there's no one section a save could isolate
/// to the way a repo-settings sub-form does.
#[test]
fn settings_body_uses_the_standard_sidebar_layout() {
    let settings = ServerSettings::default();
    let html = settings_body(&settings, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<div class="settings-layout">"#), "{html}");
    assert!(html.contains(r#"<nav class="settings-sidebar""#), "{html}");
    assert!(html.contains(r#"<div class="settings-content">"#), "{html}");
    for (id, label) in [
        ("settings-footer", "footer"),
        ("settings-network", "network"),
        ("settings-mirrors-cache", "mirrors cache"),
        ("settings-external-mirrors", "external mirrors"),
        ("settings-oidc", "OIDC login"),
    ] {
        assert!(
            html.contains(&format!(r##"<a href="#{id}">{label}</a>"##)),
            "missing sidebar link to {id:?} in {html}"
        );
        assert!(
            html.contains(&format!(r#"<div id="{id}">"#)),
            "missing anchor {id:?} in {html}"
        );
    }
}
