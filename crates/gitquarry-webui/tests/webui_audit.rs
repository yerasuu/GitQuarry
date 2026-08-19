use gitquarry_core::storage::audit::AuditEntry;
use gitquarry_i18n::Locale;
use gitquarry_webui::webui::audit::*;

#[test]
fn renders_entries_in_given_order() {
    let entries = vec![AuditEntry {
        at: "2024-01-01T00:00:00Z".to_string(),
        actor: "alice".to_string(),
        action: "repo.delete".to_string(),
        target: "acme/proj".to_string(),
        detail: None,
    }];
    let html = render_audit_page(&entries, Locale::En).into_string();
    assert!(html.contains("alice"));
    assert!(html.contains("repo.delete"));
    assert!(html.contains("acme/proj"));
}

#[test]
fn empty_shows_hint() {
    assert!(render_audit_page(&[], Locale::En).into_string().contains("empty"));
}
