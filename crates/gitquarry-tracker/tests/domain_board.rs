mod common;

use common::{test_state, test_target};
use gitquarry_tracker::domain::board::*;
use gitquarry_tracker::domain::issue;
use gitquarry_tracker::storage::board::CardRef;

#[tokio::test]
async fn get_on_a_fresh_repo_is_an_empty_board() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let (board, etag) = get(&state, &target).await.unwrap();
    assert!(board.columns.is_empty());
    assert!(etag.is_none());
}

#[tokio::test]
async fn add_column_rejects_an_empty_name() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    assert!(add_column(&state, &target, "   ").await.is_err());
}

#[tokio::test]
async fn add_column_then_get_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    add_column(&state, &target, "todo").await.unwrap();
    add_column(&state, &target, "done").await.unwrap();
    let (board, _etag) = get(&state, &target).await.unwrap();
    assert_eq!(board.columns.len(), 2);
    assert_eq!(board.columns[0].name, "todo");
    assert_eq!(board.columns[1].name, "done");
}

#[tokio::test]
async fn add_card_rejects_a_dangling_issue_number() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    add_column(&state, &target, "todo").await.unwrap();
    let err = add_card(&state, &target, 0, CardRef::Issue(999), gitquarry_i18n::Locale::En)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("doesn't exist"));
}

#[tokio::test]
async fn add_card_accepts_a_real_issue_and_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let created_issue = issue::create(&state, &target, "bug", "body", "alice").await.unwrap();
    add_column(&state, &target, "todo").await.unwrap();

    add_card(
        &state,
        &target,
        0,
        CardRef::Issue(created_issue.number),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    add_card(
        &state,
        &target,
        0,
        CardRef::Note("write docs".to_string()),
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let (board, _etag) = get(&state, &target).await.unwrap();
    assert_eq!(board.columns[0].cards.len(), 2);
    assert_eq!(board.columns[0].cards[0].card, CardRef::Issue(created_issue.number));
    assert_eq!(board.columns[0].cards[1].card, CardRef::Note("write docs".to_string()));
    // Ids are minted sequentially and never reused.
    assert_eq!(board.next_card_id, 2);
}

#[tokio::test]
async fn add_card_accepts_a_real_pull_request() {
    let dir = tempfile::tempdir().unwrap();
    let (state, fake) = test_state(dir.path()).await;
    let target = test_target();
    fake.add_pr(7);
    add_column(&state, &target, "todo").await.unwrap();

    add_card(&state, &target, 0, CardRef::PullRequest(7), gitquarry_i18n::Locale::En)
        .await
        .unwrap();

    let err = add_card(&state, &target, 0, CardRef::PullRequest(8), gitquarry_i18n::Locale::En)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("doesn't exist"));
}

#[tokio::test]
async fn move_card_to_column_relocates_it() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let locale = gitquarry_i18n::Locale::En;
    add_column(&state, &target, "todo").await.unwrap();
    add_column(&state, &target, "done").await.unwrap();
    add_card(&state, &target, 0, CardRef::Note("task".to_string()), locale)
        .await
        .unwrap();

    move_card_to_column(&state, &target, 0, 1, locale).await.unwrap();

    let (board, _etag) = get(&state, &target).await.unwrap();
    assert!(board.columns[0].cards.is_empty());
    assert_eq!(board.columns[1].cards.len(), 1);
    assert_eq!(board.columns[1].cards[0].id, 0);
}

#[tokio::test]
async fn move_card_up_and_down_swap_neighbors_and_are_a_noop_at_the_edge() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let locale = gitquarry_i18n::Locale::En;
    add_column(&state, &target, "todo").await.unwrap();
    add_card(&state, &target, 0, CardRef::Note("first".to_string()), locale)
        .await
        .unwrap();
    add_card(&state, &target, 0, CardRef::Note("second".to_string()), locale)
        .await
        .unwrap();

    // id 0 ("first") is at position 0 already — moving up is a no-op.
    move_card_up(&state, &target, 0, locale).await.unwrap();
    let (board, _etag) = get(&state, &target).await.unwrap();
    assert_eq!(board.columns[0].cards[0].card, CardRef::Note("first".to_string()));

    move_card_down(&state, &target, 0, locale).await.unwrap();
    let (board, _etag) = get(&state, &target).await.unwrap();
    assert_eq!(board.columns[0].cards[0].card, CardRef::Note("second".to_string()));
    assert_eq!(board.columns[0].cards[1].card, CardRef::Note("first".to_string()));
}

#[tokio::test]
async fn remove_card_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let locale = gitquarry_i18n::Locale::En;
    add_column(&state, &target, "todo").await.unwrap();
    add_card(&state, &target, 0, CardRef::Note("task".to_string()), locale)
        .await
        .unwrap();

    remove_card(&state, &target, 0, locale).await.unwrap();

    let (board, _etag) = get(&state, &target).await.unwrap();
    assert!(board.columns[0].cards.is_empty());
}

#[tokio::test]
async fn remove_column_rejects_a_non_empty_column() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let locale = gitquarry_i18n::Locale::En;
    add_column(&state, &target, "todo").await.unwrap();
    add_card(&state, &target, 0, CardRef::Note("task".to_string()), locale)
        .await
        .unwrap();

    assert!(remove_column(&state, &target, 0, locale).await.is_err());
}

#[tokio::test]
async fn remove_column_deletes_an_empty_column() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let locale = gitquarry_i18n::Locale::En;
    add_column(&state, &target, "todo").await.unwrap();
    add_column(&state, &target, "done").await.unwrap();

    remove_column(&state, &target, 0, locale).await.unwrap();

    let (board, _etag) = get(&state, &target).await.unwrap();
    assert_eq!(board.columns.len(), 1);
    assert_eq!(board.columns[0].name, "done");
}
