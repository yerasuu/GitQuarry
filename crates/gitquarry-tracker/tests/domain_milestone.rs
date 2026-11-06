mod common;

use common::{test_state, test_target};
use gitquarry_tracker::domain::milestone::*;
use gitquarry_tracker::storage::milestones::MilestoneStatus;

#[tokio::test]
async fn create_rejects_an_empty_title() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    assert!(create(&state, &target, "   ", "d", "").await.is_err());
}

#[tokio::test]
async fn create_then_get_roundtrips_and_numbers_increment() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();

    let first = create(&state, &target, "v1.0", "first stable", "2026-12-01")
        .await
        .unwrap();
    let second = create(&state, &target, "v2.0", "next", "").await.unwrap();
    assert_eq!(first.number, 1);
    assert_eq!(second.number, 2);

    let fetched = get(&state, &target, 1).await.unwrap().unwrap();
    assert_eq!(fetched.title, "v1.0");
    assert_eq!(fetched.due_date, "2026-12-01");
    assert_eq!(fetched.status, MilestoneStatus::Open);
}

#[tokio::test]
async fn list_returns_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    create(&state, &target, "v1.0", "", "").await.unwrap();
    create(&state, &target, "v2.0", "", "").await.unwrap();

    let all = list(&state, &target).await.unwrap();
    assert_eq!(all.iter().map(|m| m.number).collect::<Vec<_>>(), vec![2, 1]);
}

#[tokio::test]
async fn close_then_reopen_roundtrips_status() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let milestone = create(&state, &target, "v1.0", "", "").await.unwrap();

    close(&state, &target, milestone.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(
        get(&state, &target, milestone.number).await.unwrap().unwrap().status,
        MilestoneStatus::Closed
    );

    reopen(&state, &target, milestone.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert_eq!(
        get(&state, &target, milestone.number).await.unwrap().unwrap().status,
        MilestoneStatus::Open
    );
}

#[tokio::test]
async fn closing_an_already_closed_milestone_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let milestone = create(&state, &target, "v1.0", "", "").await.unwrap();

    close(&state, &target, milestone.number, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    assert!(
        close(&state, &target, milestone.number, gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn transition_on_a_nonexistent_number_errs() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    assert!(close(&state, &target, 999, gitquarry_i18n::Locale::En).await.is_err());
}

#[tokio::test]
async fn edit_updates_title_description_and_due_date() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let milestone = create(&state, &target, "v1.0", "first cut", "2026-01-01")
        .await
        .unwrap();

    edit(
        &state,
        &target,
        milestone.number,
        "v1.0.1",
        "patch release",
        "2026-02-01",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let updated = get(&state, &target, milestone.number).await.unwrap().unwrap();
    assert_eq!(updated.title, "v1.0.1");
    assert_eq!(updated.description, "patch release");
    assert_eq!(updated.due_date, "2026-02-01");
}

#[tokio::test]
async fn edit_rejects_an_empty_title() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    let milestone = create(&state, &target, "v1.0", "", "").await.unwrap();

    assert!(
        edit(
            &state,
            &target,
            milestone.number,
            "   ",
            "",
            "",
            gitquarry_i18n::Locale::En,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn edit_on_a_nonexistent_number_errs() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _fake) = test_state(dir.path()).await;
    let target = test_target();
    assert!(
        edit(&state, &target, 999, "t", "d", "", gitquarry_i18n::Locale::En)
            .await
            .is_err()
    );
}
