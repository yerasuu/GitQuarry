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
