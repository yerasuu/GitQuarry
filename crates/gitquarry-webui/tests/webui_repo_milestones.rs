#![cfg(feature = "tracker")]

mod common;

use axum::http::HeaderMap;
use common::{setup, tracker_target};
use gitquarry_i18n::Locale;
use gitquarry_webui::webui::repo::milestones::*;

#[tokio::test]
async fn milestones_list_shows_hint_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, _tstate) = setup(dir.path()).await;
    let html = render_milestones_list(&ctx, &target, "team/proj", &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("no open milestones"));
    assert!(html.contains("no closed milestones"));
}

#[tokio::test]
async fn milestones_list_shows_each_milestone_split_by_status() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::milestone::create(&tstate, &ttarget, "v1.0", "first stable", "2026-12-01")
        .await
        .unwrap();
    let closed = gitquarry_tracker::domain::milestone::create(&tstate, &ttarget, "v0.9", "beta", "")
        .await
        .unwrap();
    gitquarry_tracker::domain::milestone::close(&tstate, &ttarget, closed.number, Locale::En)
        .await
        .unwrap();

    let html = render_milestones_list(&ctx, &target, "team/proj", &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("v1.0"), "{html}");
    assert!(html.contains("v0.9"), "{html}");
    assert!(html.contains("2026-12-01"), "{html}");
    // Anonymous viewer: no manage forms (create/close/reopen).
    assert!(!html.contains(r#"action="/_milestones""#));
    assert!(!html.contains(r#"action="/_milestones/close""#));
}

#[test]
fn milestone_select_marks_the_current_milestone_selected() {
    let milestones = vec![
        gitquarry_wire::tracker::MilestoneSummaryDto {
            number: 1,
            title: "v1.0".to_string(),
            ..Default::default()
        },
        gitquarry_wire::tracker::MilestoneSummaryDto {
            number: 2,
            title: "v2.0".to_string(),
            ..Default::default()
        },
    ];
    let html = render_milestone_select(&milestones, Some(2), Locale::En).into_string();
    assert!(html.contains(r#"value="2" selected"#), "{html}");
    assert!(!html.contains(r#"value="1" selected"#), "{html}");
}
