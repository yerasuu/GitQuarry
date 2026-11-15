#![cfg(feature = "tracker")]
//! The issues list, issue detail and new-issue views, driven through
//! `webui::router` against a real tracker.

use axum::http::StatusCode;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Page, Session, TrackerFixture, tracker_fixture};
use std::sync::Arc;

async fn get(f: &TrackerFixture, query: &str) -> Page {
    let app = gitquarry_webui::webui::router(Arc::clone(&f.ctx));
    Session::get_as(&app, &format!("/team/proj?{query}"), None).await
}

async fn view(f: &TrackerFixture, query: &str) -> String {
    let page = get(f, query).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    page.body
}

#[tokio::test]
async fn issue_routes_say_why_they_503_without_a_configured_tracker() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    gitquarry_core::domain::repo::create(
        &ctx,
        "team/proj",
        None,
        None,
        None,
        gitquarry_core::storage::entities::Visibility::Public,
    )
    .await
    .unwrap()
    .unwrap();
    gitquarry_test_support::user(&ctx, "alice", true).await;
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    let session = Session::login(&app, "alice").await;

    let page = session
        .post(&app, "/_issues", &[("repo", "team/proj"), ("title", "x")])
        .await;
    assert_eq!(page.status, StatusCode::SERVICE_UNAVAILABLE, "{}", page.body);
    assert!(
        page.body
            .contains(&gitquarry_i18n::shared::tracker::tracker_not_configured(Locale::En)),
        "{}",
        page.body
    );
}

#[tokio::test]
async fn issues_list_shows_hint_when_empty() {
    let f = tracker_fixture(&[]).await;
    let html = view(&f, "view=issues").await;
    assert!(html.contains("no issues yet"));
}

#[tokio::test]
async fn issues_list_shows_each_issue_linked_by_number() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "First bug", "b", "alice")
        .await
        .unwrap();

    let html = view(&f, "view=issues").await;
    assert!(html.contains("First bug"));
    assert!(html.contains(r#"number=1"#));
}

#[tokio::test]
async fn issues_list_q_filters_by_title_or_body_case_insensitively() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Fix login bug", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::create(
        &f.tracker,
        &f.target,
        "Add dark mode",
        "mentions LOGIN flow too",
        "alice",
    )
    .await
    .unwrap();
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Unrelated", "nothing here", "alice")
        .await
        .unwrap();

    let html = view(&f, "view=issues&q=login").await;
    assert!(html.contains("Fix login bug"), "{html}");
    assert!(html.contains("Add dark mode"), "{html}");
    assert!(!html.contains("Unrelated"), "{html}");
}

#[tokio::test]
async fn issues_list_q_with_no_matches_shows_the_no_matches_hint() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Something", "else", "alice")
        .await
        .unwrap();

    let html = view(&f, "view=issues&q=nope").await;
    assert!(html.contains("no matches"), "{html}");
    assert!(!html.contains("Something"), "{html}");
}

#[tokio::test]
async fn issues_list_hides_bulk_actions_from_an_anonymous_viewer() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "First bug", "b", "alice")
        .await
        .unwrap();

    let html = view(&f, "view=issues").await;
    assert!(!html.contains(r#"action="/_issues/bulk""#), "{html}");
    // Scoped to the bulk-select checkbox specifically (`name="numbers"`),
    // not `type="checkbox"` generally -- the status filter's own mobile
    // menu toggle (`page_chrome::tab_links`) is a real, unrelated checkbox
    // that renders for every viewer regardless of permission.
    assert!(!html.contains(r#"name="numbers""#), "{html}");
}

#[tokio::test]
async fn issue_detail_404s_without_an_issue() {
    let f = tracker_fixture(&[]).await;
    assert_eq!(get(&f, "view=issue&number=99").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn issue_detail_hides_manage_forms_from_an_anonymous_viewer() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Add widget", "steps", "alice")
        .await
        .unwrap();

    let html = view(&f, "view=issue&number=1").await;
    assert!(html.contains("Add widget"));
    assert!(html.contains("steps"));
    assert!(html.contains("no comments yet"));
    assert!(!html.contains(r#"action="/_issues/close""#));
    assert!(!html.contains(r#"action="/_issues/edit""#));
    assert!(!html.contains(r#"action="/_issues/comment""#));
}

#[tokio::test]
async fn issue_detail_shows_existing_comments() {
    let f = tracker_fixture(&[]).await;
    let issue = gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Add widget", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::add_comment(
        &f.tracker,
        &f.target,
        issue.number,
        "bob",
        "looking into it",
        Locale::En,
    )
    .await
    .unwrap();

    let html = view(&f, &format!("view=issue&number={}", issue.number)).await;
    assert!(html.contains("looking into it"));
}

#[tokio::test]
async fn new_issue_form_posts_to_issues() {
    let f = tracker_fixture(&[]).await;
    let html = view(&f, "view=new_issue").await;
    assert!(html.contains(r#"action="/_issues""#));
    assert!(html.contains(r#"name="title""#));
    assert!(html.contains(r#"name="body""#));
}

#[tokio::test]
async fn new_issue_form_prefills_the_body_from_a_template() {
    let f = tracker_fixture(&[("ISSUE_TEMPLATE.md", "## Steps to reproduce")]).await;
    let html = view(&f, "view=new_issue").await;
    assert!(html.contains("## Steps to reproduce"));
}

#[tokio::test]
async fn new_issue_form_body_is_empty_without_a_template() {
    let f = tracker_fixture(&[]).await;
    let html = view(&f, "view=new_issue").await;
    assert!(
        html.contains(r#"<textarea name="body" rows="4" cols="60"></textarea>"#),
        "{html}"
    );
}

#[tokio::test]
async fn issue_detail_links_a_hash_mention_that_resolves_to_another_issue() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "First", "the original bug", "alice")
        .await
        .unwrap();
    let second =
        gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Second", "see #1 for context", "alice")
            .await
            .unwrap();

    let html = view(&f, &format!("view=issue&number={}", second.number)).await;
    assert!(
        html.contains(r#"<a href="/team/proj?view=issue&amp;number=1">#1</a>"#),
        "{html}"
    );
}

#[tokio::test]
async fn issue_detail_leaves_an_unresolved_hash_mention_as_plain_text() {
    let f = tracker_fixture(&[]).await;
    let issue = gitquarry_tracker::domain::issue::create(
        &f.tracker,
        &f.target,
        "First",
        "references #999, nothing there",
        "alice",
    )
    .await
    .unwrap();

    let html = view(&f, &format!("view=issue&number={}", issue.number)).await;
    assert!(html.contains("references #999, nothing there"), "{html}");
    assert!(!html.contains(r#"number=999"#), "{html}");
}

#[tokio::test]
async fn issue_detail_shows_its_assigned_milestone() {
    let f = tracker_fixture(&[]).await;
    let milestone = gitquarry_tracker::domain::milestone::create(&f.tracker, &f.target, "v1.0", "", "")
        .await
        .unwrap();
    let issue = gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Bug", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::edit(
        &f.tracker,
        &f.target,
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

    let html = view(&f, &format!("view=issue&number={}", issue.number)).await;
    assert!(html.contains("v1.0"), "{html}");
}

#[tokio::test]
async fn issue_detail_wraps_actions_in_an_async_fragment_target() {
    let f = tracker_fixture(&[]).await;
    gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Bug", "b", "alice")
        .await
        .unwrap();

    let html = view(&f, "view=issue&number=1").await;
    assert!(html.contains(r#"id="issue-actions""#), "{html}");
}

/// An issue by alice carrying one comment of hers, the comment's id, and
/// alice logged in.
async fn commented_issue(f: &TrackerFixture) -> (axum::Router, Session, u64, String) {
    let issue = gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "Bug", "b", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::issue::add_comment(&f.tracker, &f.target, issue.number, "alice", "mine", Locale::En)
        .await
        .unwrap();
    let (comment_id, _) = gitquarry_tracker::domain::issue::list_comments(&f.tracker, &f.target, issue.number)
        .await
        .unwrap()
        .remove(0);
    gitquarry_test_support::user(&f.ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(Arc::clone(&f.ctx));
    let session = Session::login(&app, "alice").await;
    (app, session, issue.number, comment_id)
}

#[tokio::test]
async fn comment_delete_rejects_a_comment_id_that_rewrites_the_tracker_url() {
    let f = tracker_fixture(&[]).await;
    let (app, session, number, comment_id) = commented_issue(&f).await;
    let number_field = number.to_string();
    let payload = format!("{comment_id}/delete?uid={}#", f.repo.uid);

    for route in ["/_issues/comment/delete", "/_issues/comment/edit"] {
        let fields = [
            ("repo", "team/proj"),
            ("number", number_field.as_str()),
            ("comment_id", payload.as_str()),
            ("body", "edited"),
        ];
        let page = session.post(&app, route, &fields).await;
        assert_eq!(page.status, StatusCode::BAD_REQUEST, "{route}: {}", page.body);
    }
    let comments = gitquarry_tracker::domain::issue::list_comments(&f.tracker, &f.target, number)
        .await
        .unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].1.body, "mine");
}

#[tokio::test]
async fn comment_delete_with_a_valid_id_removes_the_comment() {
    let f = tracker_fixture(&[]).await;
    let (app, session, number, comment_id) = commented_issue(&f).await;
    let number_field = number.to_string();

    let fields = [
        ("repo", "team/proj"),
        ("number", number_field.as_str()),
        ("comment_id", comment_id.as_str()),
    ];
    let page = session.post(&app, "/_issues/comment/delete", &fields).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER, "{}", page.body);
    let comments = gitquarry_tracker::domain::issue::list_comments(&f.tracker, &f.target, number)
        .await
        .unwrap();
    assert!(comments.is_empty());
}

#[tokio::test]
async fn issue_edit_shows_a_tracker_refusal_verbatim() {
    let f = tracker_fixture(&[]).await;
    let (app, session, number, _) = commented_issue(&f).await;
    let number_field = number.to_string();
    let fields = [("repo", "team/proj"), ("number", number_field.as_str()), ("title", " ")];
    let page = session.post(&app, "/_issues/edit", &fields).await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST, "{}", page.body);
    assert!(page.body.contains("issue title"), "{}", page.body);
}

#[tokio::test]
async fn issue_edit_hides_a_tracker_failure_behind_the_generic_message() {
    // The fixture's tracker reaches core at a port nothing listens on, so
    // validating a label fails inside tracker.
    let f = tracker_fixture(&[]).await;
    let (app, session, number, _) = commented_issue(&f).await;
    let number_field = number.to_string();
    let fields = [
        ("repo", "team/proj"),
        ("number", number_field.as_str()),
        ("title", "Bug"),
        ("labels", "bug"),
    ];
    let page = session.post(&app, "/_issues/edit", &fields).await;
    assert_eq!(page.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", page.body);
    assert!(
        page.body.contains(&gitquarry_i18n::shared::internal_error(Locale::En)),
        "{}",
        page.body
    );
    assert!(!page.body.contains("127.0.0.1"), "{}", page.body);
    assert!(!page.body.contains("gitquarry-core"), "{}", page.body);
}
