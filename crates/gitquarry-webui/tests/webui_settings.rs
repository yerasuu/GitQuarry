use gitquarry_core::storage::entities::ServerSettings;
use gitquarry_webui::settings::web::{settings_body, settings_full_page};

/// Each section wraps a `#<section>-body` div — that id is what
/// `data-async-target` on its own form points at (the flash banner sits
/// outside the form, so the default "target is the form itself" wouldn't
/// refresh it) — regression coverage for a submit that silently goes back
/// to a full-page POST (and brings back the "confirm resubmission" dialog
/// on F5) if either the id or the form's own attributes go missing.
#[test]
fn each_section_wraps_the_flash_and_form_in_one_async_target() {
    let settings = ServerSettings::default();
    let cases: &[(&str, &str)] = &[
        ("footer", "/_settings/footer"),
        ("network", "/_settings/network"),
        ("mirrors-cache", "/_settings/mirrors-cache"),
        ("external-mirrors", "/_settings/external-mirrors"),
        ("oidc", "/_settings/oidc"),
    ];
    for (id, action) in cases {
        let html = settings_body(id, &settings, "tok", None, gitquarry_i18n::Locale::En).into_string();
        let wrapper = format!(r#"<div id="{id}-body">"#);
        assert!(html.starts_with(&wrapper), "{id}: {html}");
        let form = format!(r##"<form method="post" action="{action}" data-async data-async-target="#{id}-body">"##);
        assert!(html.contains(&form), "{id}: missing {form:?} in {html}");
    }
}

/// Regression: every `/_settings/<section>` page is wrapped in the same
/// `.settings-layout`/`.settings-sidebar`/`.settings-content` shell
/// `account_settings_page`/`repo_settings::render_settings_section` use —
/// the standard layout for every config page in the app. Unlike a repo's
/// `?view=` sub-pages, each of these five is its own real route with its
/// own CAS/validation scoped to just that section's fields.
#[test]
fn settings_page_uses_the_standard_sidebar_layout() {
    let settings = ServerSettings::default();
    let html = settings_full_page("network", &settings, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<div class="settings-layout">"#), "{html}");
    assert!(html.contains(r#"<nav class="settings-sidebar""#), "{html}");
    assert!(html.contains(r#"<div class="settings-content">"#), "{html}");
    for (id, href, label) in [
        ("footer", "/_settings/footer", "footer"),
        ("network", "/_settings/network", "network"),
        ("mirrors-cache", "/_settings/mirrors-cache", "mirrors cache"),
        ("external-mirrors", "/_settings/external-mirrors", "external mirrors"),
        ("oidc", "/_settings/oidc", "OIDC login"),
    ] {
        assert!(
            html.contains(&format!(r##"<a href="{href}">{label}</a>"##))
                || html.contains(&format!(
                    r##"<a class="active" aria-current="page" href="{href}">{label}</a>"##
                )),
            "missing sidebar link to {id:?} in {html}"
        );
    }
}

/// The active section's sidebar entry is the one marked `class="active"
/// aria-current="page"` — without this an admin has no visual cue for
/// which of the five pages they're on beyond the URL bar.
#[test]
fn active_section_is_marked_in_the_sidebar() {
    let settings = ServerSettings::default();
    let html = settings_full_page("oidc", &settings, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(
        html.contains(r##"<a class="active" aria-current="page" href="/_settings/oidc">OIDC login</a>"##),
        "{html}"
    );
}
