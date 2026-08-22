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

/// Regression for the low-value filter `render_activity` runs before
/// handing events to [`render_events`]: personal-preference toggles and
/// infra noise drop out, everything that actually happened to the repo
/// stays.
#[test]
fn filter_low_value_drops_preference_toggles_and_noise_but_keeps_real_activity() {
    fn entry(action: &str) -> AuditEntry {
        AuditEntry {
            at: "2024-01-01T00:00:00Z".to_string(),
            actor: "alice".to_string(),
            action: action.to_string(),
            target: "acme/proj".to_string(),
            detail: None,
        }
    }
    let events = vec![
        entry("repo.star"),
        entry("repo.unstar"),
        entry("repo.watch"),
        entry("repo.unwatch"),
        entry("mirror.sync"),
        entry("merge_request.comment"),
        entry("repo.push"),
        entry("merge_request.merge"),
        entry("release.create"),
    ];
    let filtered = filter_low_value(events);
    let kept: Vec<&str> = filtered.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(kept, vec!["repo.push", "merge_request.merge", "release.create"]);
}
