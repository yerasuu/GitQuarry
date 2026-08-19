#![cfg(feature = "tracker")]

mod common;

use axum::http::HeaderMap;
use common::{setup, tracker_target};
use gitquarry_i18n::Locale;
use gitquarry_webui::webui::repo::issues::*;

#[tokio::test]
async fn issues_list_shows_hint_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, _tstate) = setup(dir.path()).await;
    let html = render_issues_list(&ctx, &target, "team/proj", "open", None, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("no issues yet"));
}

#[tokio::test]
async fn issues_list_shows_each_issue_linked_by_number() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "First bug", "b", "alice")
        .await
        .unwrap();

    let html = render_issues_list(&ctx, &target, "team/proj", "open", None, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("First bug"));
    assert!(html.contains(r#"number=1"#));
}

#[tokio::test]
async fn issues_list_q_filters_by_title_or_body_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Fix login bug", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Add dark mode", "mentions LOGIN flow too", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Unrelated", "nothing here", "alice")
        .await
        .unwrap();

    let html = render_issues_list(
        &ctx,
        &target,
        "team/proj",
        "open",
        Some("login"),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("Fix login bug"), "{html}");
    assert!(html.contains("Add dark mode"), "{html}");
    assert!(!html.contains("Unrelated"), "{html}");
}

#[tokio::test]
async fn issues_list_q_with_no_matches_shows_the_no_matches_hint() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Something", "else", "alice")
        .await
        .unwrap();

    let html = render_issues_list(
        &ctx,
        &target,
        "team/proj",
        "open",
        Some("nope"),
        &HeaderMap::new(),
        Locale::En,
    )
    .await
    .unwrap();
    assert!(html.contains("no matches"), "{html}");
    assert!(!html.contains("Something"), "{html}");
}

#[tokio::test]
async fn issues_list_hides_bulk_actions_from_an_anonymous_viewer() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "First bug", "b", "alice")
        .await
        .unwrap();

    let html = render_issues_list(&ctx, &target, "team/proj", "open", None, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(!html.contains(r#"action="/_issues/bulk""#), "{html}");
    // Scoped to the bulk-select checkbox specifically (`name="numbers"`),
    // not `type="checkbox"` generally -- the status filter's own mobile
    // menu toggle (`page_chrome::tab_links`) is a real, unrelated checkbox
    // that renders for every viewer regardless of permission.
    assert!(!html.contains(r#"name="numbers""#), "{html}");
}

#[tokio::test]
async fn issue_detail_404s_without_an_issue() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, _tstate) = setup(dir.path()).await;
    assert!(
        render_issue_detail(&ctx, &target, "team/proj", 99, &HeaderMap::new(), Locale::En)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn issue_detail_hides_manage_forms_from_an_anonymous_viewer() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Add widget", "steps", "alice")
        .await
        .unwrap();

    let html = render_issue_detail(&ctx, &target, "team/proj", 1, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("Add widget"));
    assert!(html.contains("steps"));
    assert!(html.contains("no comments yet"));
    assert!(!html.contains(r#"action="/_issues/close""#));
    assert!(!html.contains(r#"action="/_issues/edit""#));
    assert!(!html.contains(r#"action="/_issues/comment""#));
}

#[tokio::test]
async fn issue_detail_shows_existing_comments() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    let issue = gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Add widget", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::add_comment(
        &tstate,
        &ttarget,
        issue.number,
        "bob",
        "looking into it",
        Locale::En,
    )
    .await
    .unwrap();

    let html = render_issue_detail(&ctx, &target, "team/proj", issue.number, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("looking into it"));
}

#[test]
fn new_issue_form_posts_to_issues() {
    let html = render_new_issue_form("team/proj", None, "", "", None, &HeaderMap::new(), Locale::En).into_string();
    assert!(html.contains(r#"action="/_issues""#));
    assert!(html.contains(r#"name="title""#));
    assert!(html.contains(r#"name="body""#));
}

#[test]
fn new_issue_form_prefills_the_body_from_a_template() {
    let html = render_new_issue_form(
        "team/proj",
        Some("## Steps to reproduce"),
        "",
        "",
        None,
        &HeaderMap::new(),
        Locale::En,
    )
    .into_string();
    assert!(html.contains("## Steps to reproduce"));
}

#[test]
fn new_issue_form_body_is_empty_without_a_template() {
    let html = render_new_issue_form("team/proj", None, "", "", None, &HeaderMap::new(), Locale::En).into_string();
    assert!(
        html.contains(r#"<textarea name="body" rows="4" cols="60"></textarea>"#),
        "{html}"
    );
}

#[tokio::test]
async fn issue_detail_links_a_hash_mention_that_resolves_to_another_issue() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "First", "the original bug", "alice")
        .await
        .unwrap();
    let second = gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Second", "see #1 for context", "alice")
        .await
        .unwrap();

    let html = render_issue_detail(&ctx, &target, "team/proj", second.number, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(
        html.contains(r#"<a href="/team/proj?view=issue&amp;number=1">#1</a>"#),
        "{html}"
    );
}

#[tokio::test]
async fn issue_detail_leaves_an_unresolved_hash_mention_as_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    let issue =
        gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "First", "references #999, nothing there", "alice")
            .await
            .unwrap();

    let html = render_issue_detail(&ctx, &target, "team/proj", issue.number, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("references #999, nothing there"), "{html}");
    assert!(!html.contains(r#"number=999"#), "{html}");
}

#[tokio::test]
async fn issue_detail_shows_its_assigned_milestone() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    let milestone = gitquarry_tracker::domain::milestone::create(&tstate, &ttarget, "v1.0", "", "")
        .await
        .unwrap();
    let issue = gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Bug", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::edit(
        &tstate,
        &ttarget,
        issue.number,
        gitquarry_tracker::domain::issue::IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: Some(milestone.number),
        },
        Locale::En,
    )
    .await
    .unwrap();

    let html = render_issue_detail(&ctx, &target, "team/proj", issue.number, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("v1.0"), "{html}");
}

#[tokio::test]
async fn issue_detail_wraps_actions_in_an_async_fragment_target() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "Bug", "b", "alice")
        .await
        .unwrap();

    let html = render_issue_detail(&ctx, &target, "team/proj", 1, &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains(r#"id="issue-actions""#), "{html}");
}
