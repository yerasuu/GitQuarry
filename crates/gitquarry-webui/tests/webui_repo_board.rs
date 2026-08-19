#![cfg(feature = "tracker")]

mod common;

use axum::http::HeaderMap;
use common::{setup, tracker_target};
use gitquarry_i18n::Locale;
use gitquarry_tracker::storage::board::CardRef;
use gitquarry_webui::webui::repo::board::*;

#[tokio::test]
async fn board_shows_hint_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, _tstate) = setup(dir.path()).await;
    let html = render_board(&ctx, &target, "team/proj", &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("no columns yet"), "{html}");
    // Anonymous viewer: no manage forms.
    assert!(!html.contains(r#"action="/_board/add_column""#));
}

#[tokio::test]
async fn board_shows_columns_and_cards() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let ttarget = tracker_target(&target);
    let issue = gitquarry_tracker::domain::issue::create(&tstate, &ttarget, "fix crash", "body", "alice")
        .await
        .unwrap();
    gitquarry_tracker::domain::board::add_column(&tstate, &ttarget, "todo")
        .await
        .unwrap();
    gitquarry_tracker::domain::board::add_card(&tstate, &ttarget, 0, CardRef::Issue(issue.number), Locale::En)
        .await
        .unwrap();
    gitquarry_tracker::domain::board::add_card(
        &tstate,
        &ttarget,
        0,
        CardRef::Note("write docs".to_string()),
        Locale::En,
    )
    .await
    .unwrap();

    let html = render_board(&ctx, &target, "team/proj", &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("todo"), "{html}");
    assert!(html.contains("fix crash"), "{html}");
    assert!(html.contains("write docs"), "{html}");
    // Column carries its drop-target index unconditionally (drag-and-drop
    // needs it regardless of viewer permission — only `draggable` on the
    // card itself is manage-gated, see the negative assertion below).
    assert!(html.contains(r#"data-column-index="0""#), "{html}");
    // Anonymous viewer can't manage the board, so cards aren't draggable
    // either — same gate the move-up/down/to-column forms already use.
    // (The `can_manage = true` branch has no test coverage in this file at
    // all yet — it needs a real authenticated session, which no test here
    // currently sets up.)
    assert!(!html.contains("draggable="), "{html}");
}

#[tokio::test]
async fn board_shows_a_broken_reference_hint_for_a_dangling_card() {
    // `gitquarry_tracker::domain::board::add_card` always validates its target exists, so a
    // dangling card can't be created through the normal API — this writes
    // the board directly, bypassing that check, the same way a card could
    // in principle end up dangling if whatever it once pointed at were
    // later removed by some future feature that doesn't clean up cards.
    let dir = tempfile::tempdir().unwrap();
    let (ctx, target, tstate) = setup(dir.path()).await;
    let board = gitquarry_tracker::storage::board::Board {
        columns: vec![gitquarry_tracker::storage::board::BoardColumn {
            name: "todo".to_string(),
            cards: vec![gitquarry_tracker::storage::board::BoardCard {
                id: 0,
                card: CardRef::Issue(999),
            }],
        }],
        next_card_id: 1,
    };
    gitquarry_tracker::storage::board::put_board(tstate.backend.as_ref(), &tstate.cipher, &target.uid, &board, None)
        .await
        .unwrap();

    let html = render_board(&ctx, &target, "team/proj", &HeaderMap::new(), Locale::En)
        .await
        .unwrap();
    assert!(html.contains("broken reference"), "{html}");
}
