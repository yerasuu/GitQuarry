#![cfg(feature = "tracker")]
//! The board view, driven through `webui::router` against a real tracker.

use axum::http::StatusCode;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Session, TrackerFixture, tracker_fixture};
use gitquarry_tracker::storage::board::CardRef;
use std::sync::Arc;

async fn board(f: &TrackerFixture) -> String {
    let app = gitquarry_webui::webui::router(Arc::clone(&f.ctx));
    let page = Session::get_as(&app, "/team/proj?view=board", None).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    page.body
}

#[tokio::test]
async fn board_shows_hint_when_empty() {
    let f = tracker_fixture(&[]).await;
    let html = board(&f).await;
    assert!(html.contains("no columns yet"), "{html}");
    // Anonymous viewer: no manage forms.
    assert!(!html.contains(r#"action="/_board/add_column""#));
}

#[tokio::test]
async fn board_shows_columns_and_cards() {
    let f = tracker_fixture(&[]).await;
    let issue = gitquarry_tracker::domain::issue::create(&f.tracker, &f.target, "fix crash", "body", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::board::add_column(&f.tracker, &f.target, "todo")
        .await
        .unwrap();
    gitquarry_tracker::domain::board::add_card(&f.tracker, &f.target, 0, CardRef::Issue(issue.number), Locale::En)
        .await
        .unwrap();
    gitquarry_tracker::domain::board::add_card(
        &f.tracker,
        &f.target,
        0,
        CardRef::Note("write docs".to_string()),
        Locale::En,
    )
    .await
    .unwrap();

    let html = board(&f).await;
    assert!(html.contains("todo"), "{html}");
    assert!(html.contains("fix crash"), "{html}");
    assert!(html.contains("write docs"), "{html}");
    // Column carries its drop-target index unconditionally (drag-and-drop
    // needs it regardless of viewer permission — only `draggable` on the
    // card itself is manage-gated, see the negative assertion below).
    assert!(html.contains(r#"data-column-index="0""#), "{html}");
    // Anonymous viewer can't manage the board, so cards aren't draggable
    // either — same gate the move-up/down/to-column forms already use.
    assert!(!html.contains("draggable="), "{html}");
}

#[tokio::test]
async fn board_shows_a_broken_reference_hint_for_a_dangling_card() {
    // `gitquarry_tracker::domain::board::add_card` always validates its
    // target exists, so a dangling card can't be created through the normal
    // API — this writes the board directly, bypassing that check, the same
    // way a card could end up dangling if whatever it pointed at were later
    // removed without cleaning up cards.
    let f = tracker_fixture(&[]).await;
    let board_state = gitquarry_tracker::storage::board::Board {
        columns: vec![gitquarry_tracker::storage::board::BoardColumn {
            name: "todo".to_string(),
            cards: vec![gitquarry_tracker::storage::board::BoardCard {
                id: 0,
                card: CardRef::Issue(999),
            }],
        }],
        next_card_id: 1,
    };
    gitquarry_tracker::storage::board::put_board(
        f.tracker.backend.as_ref(),
        &f.tracker.cipher,
        &f.repo.uid,
        &board_state,
        None,
    )
    .await
    .unwrap();

    let html = board(&f).await;
    assert!(html.contains("broken reference"), "{html}");
}
