use gitquarry_i18n::Locale;
use gitquarry_webui::webui::ci::{PendingRun, render_pending_page};

#[test]
fn renders_a_row_per_pending_run_with_an_approve_form() {
    let runs = vec![PendingRun {
        id: "deadbeef-deploy".to_string(),
        job_id: "deploy".to_string(),
        kind: "deploy".to_string(),
        sha: "deadbeef".to_string(),
    }];
    let html = render_pending_page(
        "acme/widgets",
        &runs,
        "<input type=\"hidden\" name=\"csrf\" value=\"tok\">",
        Locale::En,
    )
    .into_string();
    assert!(html.contains("deploy"));
    assert!(html.contains("deadbeef"));
    assert!(html.contains("name=\"run_id\" value=\"deadbeef-deploy\""));
    assert!(html.contains("action=\"/_ci/approve\""));
    assert!(html.contains("name=\"csrf\" value=\"tok\""));
}

#[test]
fn empty_shows_hint() {
    assert!(
        render_pending_page("acme/widgets", &[], "", Locale::En)
            .into_string()
            .contains("no deploys are waiting")
    );
}
