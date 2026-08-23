//! `/_settings/backup` — the form, and the run history it renders beneath it.

use gitquarry_core::backup::inventory::Class;
use gitquarry_core::backup::report::{BackupRun, Trigger};
use gitquarry_core::storage::entities::ServerSettings;
use gitquarry_i18n::Locale;
use gitquarry_webui::settings::web::settings_backup_body;

fn render(settings: &ServerSettings, runs: &[BackupRun]) -> String {
    settings_backup_body(settings, runs, "tok", None, Locale::En).into_string()
}

#[test]
fn an_unconfigured_instance_shows_the_defaults_the_job_would_use() {
    let html = render(&ServerSettings::default(), &[]);

    // Off until someone turns it on: a backup writes somewhere an operator
    // hasn't necessarily chosen yet.
    assert!(!html.contains(r#"name="enabled" value="1" checked"#), "{html}");
    // The default selection: everything but the rebuildable caches.
    for class in [
        "class_git",
        "class_lfs",
        "class_releases",
        "class_avatars",
        "class_entities",
        "class_audit",
    ] {
        assert!(
            html.contains(&format!(r#"name="{class}" value="1" checked"#)),
            "{class} should default to on: {html}"
        );
    }
    assert!(
        html.contains(r#"name="class_derived" value="1">"#),
        "derived should default to off: {html}"
    );
    // And the snapshot half is on: it's the only half that keeps history.
    assert!(html.contains(r#"name="snapshot" value="1" checked"#), "{html}");
}

#[test]
fn a_stored_configuration_is_rendered_back_into_the_form() {
    let settings = ServerSettings {
        backup_dest: Some("bucket:offsite".to_string()),
        backup_enabled: true,
        backup_interval: Some("6h".to_string()),
        backup_include_lfs: Some(false),
        backup_include_derived: Some(true),
        backup_keep: Some(7),
        backup_audit_keep_months: Some(12),
        backup_delete_grace_days: Some(30),
        backup_concurrency: Some(16),
        ..Default::default()
    };
    let html = render(&settings, &[]);

    assert!(html.contains(r#"name="backup_dest" value="bucket:offsite""#), "{html}");
    assert!(html.contains(r#"name="enabled" value="1" checked"#), "{html}");
    assert!(html.contains(r#"name="interval" value="6h""#), "{html}");
    assert!(
        html.contains(r#"name="class_lfs" value="1">"#),
        "lfs should be off: {html}"
    );
    assert!(
        html.contains(r#"name="class_derived" value="1" checked"#),
        "derived should be on: {html}"
    );
    assert!(html.contains(r#"name="keep" value="7""#), "{html}");
    assert!(html.contains(r#"name="audit_keep_months" value="12""#), "{html}");
    assert!(html.contains(r#"name="delete_grace_days" value="30""#), "{html}");
    assert!(html.contains(r#"name="concurrency" value="16""#), "{html}");
}

#[test]
fn an_unset_numeric_field_renders_empty_rather_than_a_zero() {
    // Empty means "the documented default", and a rendered `0` would read as
    // "keep no snapshots" / "no concurrency" instead.
    let html = render(&ServerSettings::default(), &[]);
    assert!(html.contains(r#"name="keep" value="""#), "{html}");
    assert!(html.contains(r#"name="delete_grace_days" value="""#), "{html}");
}

#[test]
fn run_now_is_its_own_form_so_it_cannot_carry_unsaved_edits() {
    let html = render(&ServerSettings::default(), &[]);
    assert!(
        html.contains(r#"<form method="post" action="/_settings/backup/run">"#),
        "{html}"
    );
}

#[test]
fn the_destination_secrets_warning_is_always_shown() {
    // The replica holds the ssh host key, the signing key and every password
    // hash: an admin choosing a destination has to see that said out loud.
    let html = render(&ServerSettings::default(), &[]);
    assert!(html.contains("same secrets as this bucket"), "{html}");
}

#[test]
fn with_no_runs_the_history_says_so_instead_of_an_empty_table() {
    let html = render(&ServerSettings::default(), &[]);
    assert!(html.contains("no backup has run yet."), "{html}");
    assert!(!html.contains("<tbody>"), "{html}");
}

#[test]
fn a_finished_run_shows_its_totals() {
    let mut run = BackupRun::starting("bucket:offsite", Trigger::Scheduled);
    run.count(Class::Git, Some(2 * 1024 * 1024));
    run.count(Class::Lfs, Some(1024));
    run.count(Class::Entities, None);
    run.finish();

    let html = render(&ServerSettings::default(), &[run]);
    assert!(html.contains("<tbody>"), "{html}");
    assert!(html.contains(">ok<"), "{html}");
    assert!(html.contains(">scheduled<"), "{html}");
    // Two objects copied, rendered with a human byte total.
    assert!(html.contains("2 (2.0M)"), "{html}");
    // And the one that was already at the destination.
    assert!(html.contains(">1</td>"), "{html}");
}

#[test]
fn a_failed_run_shows_the_reason_it_never_started() {
    let mut run = BackupRun::starting("bucket:typo", Trigger::Scheduled);
    run.fail("bucket 'typo' not configured".to_string());

    let html = render(&ServerSettings::default(), &[run]);
    assert!(html.contains(">failed<"), "{html}");
    assert!(html.contains("bucket 'typo' not configured"), "{html}");
}

#[test]
fn per_object_failures_are_listed_behind_a_disclosure() {
    let mut run = BackupRun::starting("self", Trigger::Manual);
    run.count_failure(Class::Lfs, "repos/r1/lfs/abc", "connection reset");
    run.finish();

    let html = render(&ServerSettings::default(), &[run]);
    assert!(html.contains(">partial<"), "{html}");
    assert!(html.contains("<details>"), "{html}");
    assert!(html.contains("repos/r1/lfs/abc: connection reset"), "{html}");
}

#[test]
fn unclassified_keys_are_surfaced_so_they_can_be_classified() {
    // They're always copied; the point of showing them is that someone
    // should go teach `backup::inventory` what they are.
    let mut run = BackupRun::starting("self", Trigger::Cli);
    run.note_unknown("brand-new-feature/thing.json");
    run.finish();

    let html = render(&ServerSettings::default(), &[run]);
    assert!(html.contains("unclassified objects copied"), "{html}");
    assert!(html.contains("<code>brand-new-feature/thing.json</code>"), "{html}");
}

#[test]
fn a_still_running_run_shows_no_duration_rather_than_zero() {
    let run = BackupRun::starting("self", Trigger::Manual);
    let html = render(&ServerSettings::default(), &[run]);
    assert!(html.contains(">running<"), "{html}");
    assert!(html.contains("<td>-</td>"), "{html}");
}
