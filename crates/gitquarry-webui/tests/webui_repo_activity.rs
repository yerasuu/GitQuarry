use gitquarry_core::storage::audit::AuditEntry;
use gitquarry_i18n::Locale;
use gitquarry_webui::webui::repo::activity::*;

#[test]
fn renders_events_for_this_repo() {
    let events = vec![AuditEntry {
        at: "2024-01-01T00:00:00Z".to_string(),
        actor: "alice".to_string(),
        action: "repo.star".to_string(),
        target: "acme/proj".to_string(),
        detail: None,
    }];
    let html = render_events(&events, Locale::En).into_string();
    assert!(html.contains("alice"));
    assert!(html.contains("repo.star"));
}

#[test]
fn empty_shows_hint() {
    assert!(
        render_events(&[], Locale::En)
            .into_string()
            .contains("no recent events")
    );
}
