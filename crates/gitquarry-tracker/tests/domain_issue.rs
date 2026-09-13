mod common;

use common::{test_state, test_target};
use gitquarry_tracker::domain::issue::*;
use gitquarry_tracker::storage::issues::IssueStatus;
use s3db::Cas;

#[tokio::test]
async fn create_then_get_roundtrips_and_numbers_increment() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();

    let first = create(&state, &target, "First bug", "body", "alice").await.unwrap();
    let second = create(&state, &target, "Second bug", "body", "alice").await.unwrap();
    assert_eq!(first.number, 1);
    assert_eq!(second.number, 2);

    let fetched = get(&state, &target, 1).await.unwrap().unwrap();
    assert_eq!(fetched.title, "First bug");
    assert_eq!(fetched.status, IssueStatus::Open);
}

#[tokio::test]
async fn create_rejects_an_empty_title() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    assert!(create(&state, &target, "   ", "body", "alice").await.is_err());
}

#[tokio::test]
async fn list_filters_by_status() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();

    let first = create(&state, &target, "Open one", "b", "alice").await.unwrap();
    create(&state, &target, "Also open", "b", "alice").await.unwrap();
    close(&state, &target, first.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();

    let open = list(&state, &target, Some(IssueStatus::Open)).await.unwrap();
    let closed = list(&state, &target, Some(IssueStatus::Closed)).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].number, first.number);
}

#[tokio::test]
async fn close_then_reopen_roundtrips_status() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    close(&state, &target, issue.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(
        get(&state, &target, issue.number).await.unwrap().unwrap().status,
        IssueStatus::Closed
    );

    reopen(&state, &target, issue.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(
        get(&state, &target, issue.number).await.unwrap().unwrap().status,
        IssueStatus::Open
    );
}

#[tokio::test]
async fn closing_an_already_closed_issue_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    close(&state, &target, issue.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(
        close(&state, &target, issue.number, gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn add_comment_fails_for_a_nonexistent_issue() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    assert!(
        add_comment(&state, &target, 99, "alice", "hi", gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn add_comment_then_list_roundtrips_chronologically() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    add_comment(
        &state,
        &target,
        issue.number,
        "alice",
        "first",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    add_comment(
        &state,
        &target,
        issue.number,
        "bob",
        "second",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let comments = list_comments(&state, &target, issue.number).await.unwrap();
    assert_eq!(
        comments.iter().map(|(_id, c)| c.body.as_str()).collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[tokio::test]
async fn edit_rejects_an_unknown_label() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    let result = edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "nonexistent",
            assignees_csv: "",
            milestone: None,
        },
        gitquarry_i18n::Locale::En,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_rejects_an_unknown_assignee() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    let result = edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "nobody",
            milestone: None,
        },
        gitquarry_i18n::Locale::En,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_updates_title_body_labels_and_assignees() {
    let dir = tempfile::tempdir().unwrap();
    let (state, fake) = test_state(dir.path()).await;
    let target = test_target();
    fake.add_label("bug");
    fake.add_user("bob");
    let issue = create(&state, &target, "Old title", "old body", "alice").await.unwrap();

    let cas = edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "New title",
            body: "new body",
            labels_csv: "bug",
            assignees_csv: "bob",
            milestone: None,
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert_eq!(cas, Cas::Ok);

    let updated = get(&state, &target, issue.number).await.unwrap().unwrap();
    assert_eq!(updated.title, "New title");
    assert_eq!(updated.body, "new body");
    assert_eq!(updated.labels, vec!["bug".to_string()]);
    assert_eq!(updated.assignees, vec!["bob".to_string()]);
}

#[tokio::test]
async fn edit_rejects_an_unknown_milestone() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    let result = edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: Some(999),
        },
        gitquarry_i18n::Locale::En,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_assigns_and_clears_a_milestone() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let milestone = gitquarry_tracker::domain::milestone::create(&state, &target, "v1.0", "", "")
        .await
        .unwrap();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: Some(milestone.number),
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert_eq!(
        get(&state, &target, issue.number).await.unwrap().unwrap().milestone,
        Some(milestone.number)
    );

    edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: None,
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert_eq!(
        get(&state, &target, issue.number).await.unwrap().unwrap().milestone,
        None
    );
}

#[tokio::test]
async fn milestone_counts_track_assignment_status_and_reassignment() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let m1 = gitquarry_tracker::domain::milestone::create(&state, &target, "v1.0", "", "")
        .await
        .unwrap();
    let m2 = gitquarry_tracker::domain::milestone::create(&state, &target, "v2.0", "", "")
        .await
        .unwrap();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    async fn get_milestone(
        state: &gitquarry_tracker::state::TrackerState,
        target: &gitquarry_tracker::state::TrackerTarget,
        n: u64,
    ) -> gitquarry_tracker::storage::milestones::MilestoneMeta {
        gitquarry_tracker::domain::milestone::get(state, target, n)
            .await
            .unwrap()
            .unwrap()
    }

    edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: Some(m1.number),
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let m1_after = get_milestone(&state, &target, m1.number).await;
    assert_eq!((m1_after.open_issues, m1_after.closed_issues), (1, 0));

    close(&state, &target, issue.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let m1_after = get_milestone(&state, &target, m1.number).await;
    assert_eq!((m1_after.open_issues, m1_after.closed_issues), (0, 1));

    reopen(&state, &target, issue.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let m1_after = get_milestone(&state, &target, m1.number).await;
    assert_eq!((m1_after.open_issues, m1_after.closed_issues), (1, 0));

    // Reassign to a different milestone: old one drops to zero, new one
    // picks up the same open/closed bucket the issue was actually in.
    edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: Some(m2.number),
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let m1_after = get_milestone(&state, &target, m1.number).await;
    let m2_after = get_milestone(&state, &target, m2.number).await;
    assert_eq!((m1_after.open_issues, m1_after.closed_issues), (0, 0));
    assert_eq!((m2_after.open_issues, m2_after.closed_issues), (1, 0));

    // Clearing the milestone entirely drops it back to zero too.
    edit(
        &state,
        &target,
        issue.number,
        IssueEdit {
            title: "Bug",
            body: "b",
            labels_csv: "",
            assignees_csv: "",
            milestone: None,
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let m2_after = get_milestone(&state, &target, m2.number).await;
    assert_eq!((m2_after.open_issues, m2_after.closed_issues), (0, 0));
}

#[tokio::test]
async fn edit_comment_updates_the_body_but_rejects_a_non_author() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();
    add_comment(
        &state,
        &target,
        issue.number,
        "alice",
        "first",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let (comment_id, _) = &list_comments(&state, &target, issue.number).await.unwrap()[0];

    assert!(
        edit_comment(
            &state,
            &target,
            issue.number,
            comment_id,
            "bob",
            "hijacked",
            gitquarry_i18n::Locale::En,
        )
        .await
        .is_err()
    );

    assert_eq!(
        edit_comment(
            &state,
            &target,
            issue.number,
            comment_id,
            "alice",
            "edited",
            gitquarry_i18n::Locale::En,
        )
        .await
        .unwrap(),
        Cas::Ok
    );
    let comments = list_comments(&state, &target, issue.number).await.unwrap();
    assert_eq!(comments[0].1.body, "edited");
}

#[tokio::test]
async fn delete_comment_removes_it_but_rejects_a_non_author() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();
    add_comment(
        &state,
        &target,
        issue.number,
        "alice",
        "first",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let (comment_id, _) = &list_comments(&state, &target, issue.number).await.unwrap()[0];

    assert!(
        delete_comment(
            &state,
            &target,
            issue.number,
            comment_id,
            "bob",
            false,
            gitquarry_i18n::Locale::En,
        )
        .await
        .is_err()
    );

    delete_comment(
        &state,
        &target,
        issue.number,
        comment_id,
        "alice",
        false,
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(list_comments(&state, &target, issue.number).await.unwrap().is_empty());
}

/// What used to be `pull_request_auto_close.rs`'s own event-bus-consumer
/// tests before the tracker binary split — `merge`'s side of the push (that
/// it actually reaches `POST /_internal/tracker/merge-request-merged` with
/// the right title/description) is covered on `gitquarry-core`'s own side
/// now; this only needs to prove `close_referenced_by_merge_request`'s own
/// "closes #N" keyword parse, given a notification payload directly.
#[tokio::test]
async fn close_referenced_by_merge_request_closes_on_a_closing_keyword() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    close_referenced_by_merge_request(
        &state,
        &target,
        "Add widget",
        &format!("Closes: #{}", issue.number),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let closed = get(&state, &target, issue.number).await.unwrap().unwrap();
    assert_eq!(closed.status, IssueStatus::Closed);
}

#[tokio::test]
async fn close_referenced_by_merge_request_ignores_a_bare_hash_mention() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();

    close_referenced_by_merge_request(
        &state,
        &target,
        "Add widget",
        &format!("see #{} for context", issue.number),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let untouched = get(&state, &target, issue.number).await.unwrap().unwrap();
    assert_eq!(untouched.status, IssueStatus::Open);
}

#[tokio::test]
async fn delete_comment_allows_a_moderator_to_remove_someone_elses() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let issue = create(&state, &target, "Bug", "b", "alice").await.unwrap();
    add_comment(
        &state,
        &target,
        issue.number,
        "alice",
        "first",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let (comment_id, _) = &list_comments(&state, &target, issue.number).await.unwrap()[0];

    // Not the author, but the caller vouches `bob` is Developer+ on the repo.
    delete_comment(
        &state,
        &target,
        issue.number,
        comment_id,
        "bob",
        true,
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    assert!(list_comments(&state, &target, issue.number).await.unwrap().is_empty());
}
