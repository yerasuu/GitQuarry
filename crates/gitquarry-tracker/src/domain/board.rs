//! Every mutation of a repo's [`crate::storage::board::Board`] goes through
//! here — same `domain`/`web` boundary [`crate::domain::issue`]'s doc
//! comment describes: transport-agnostic, `anyhow::Result`, no CSRF/
//! session/audit concerns, those stay in `webui::repo::board`.
//!
//! One board per repo, one JSON blob (see `storage::board`'s own doc
//! comment for why), so every mutation here is a plain get-etag/mutate/
//! CAS-put against the whole thing — no retry loop, `Cas::Conflict` just
//! surfaces to the caller to show "someone else changed this, try again"
//! and re-render, same as `domain::milestone::edit`.
//!
//! Reordering is button-driven (move a card to another column, or swap it
//! with its neighbor within one), not drag-and-drop.
//!
//! `CardRef::PullRequest` cards are validated against `state.core.pr_summary`
//! — this is the one place tracker code depends on core's pull-request
//! domain, now over the loopback instead of a direct in-process call.

use crate::domain::issue;
use crate::i18n;
use crate::state::{TrackerState, TrackerTarget};
use crate::storage::board::{self, Board, BoardCard, CardRef};
use anyhow::{Result, bail};
use s3db::Cas;

pub async fn get(state: &TrackerState, target: &TrackerTarget) -> Result<(Board, Option<String>)> {
    board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await
}

pub async fn add_column(state: &TrackerState, target: &TrackerTarget, name: &str) -> Result<Cas> {
    let name = name.trim();
    if name.is_empty() {
        bail!("column name can't be empty");
    }
    let (mut board, etag) = board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    board.columns.push(crate::storage::board::BoardColumn {
        name: name.to_string(),
        cards: Vec::new(),
    });
    board::put_board(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &board,
        etag.as_deref(),
    )
    .await
}

pub async fn rename_column(
    state: &TrackerState,
    target: &TrackerTarget,
    column_index: usize,
    name: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let name = name.trim();
    if name.is_empty() {
        bail!("column name can't be empty");
    }
    let (mut board, etag) = board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let Some(column) = board.columns.get_mut(column_index) else {
        bail!("{}", i18n::board_column_not_found(locale));
    };
    column.name = name.to_string();
    board::put_board(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &board,
        etag.as_deref(),
    )
    .await
}

/// Refuses a non-empty column — see [`crate::storage::board::CardRef`]'s own
/// doc comment on why a `Note` card can't just be discarded silently.
pub async fn remove_column(
    state: &TrackerState,
    target: &TrackerTarget,
    column_index: usize,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut board, etag) = board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let Some(column) = board.columns.get(column_index) else {
        bail!("{}", i18n::board_column_not_found(locale));
    };
    if !column.cards.is_empty() {
        bail!("{}", i18n::board_column_not_empty(locale));
    }
    board.columns.remove(column_index);
    board::put_board(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &board,
        etag.as_deref(),
    )
    .await
}

/// `Issue`/`PullRequest` cards must point at a real, existing issue/PR in
/// this same repo (checked here, not left to the caller) — a dangling
/// number would otherwise render as a permanently broken link.
pub async fn add_card(
    state: &TrackerState,
    target: &TrackerTarget,
    column_index: usize,
    card: CardRef,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    match &card {
        CardRef::Issue(number) => {
            if issue::get(state, target, *number).await?.is_none() {
                bail!("{}", i18n::board_card_target_not_found(locale));
            }
        }
        CardRef::PullRequest(number) => {
            if state.core.pr_summary(target.uid.as_str(), *number).await?.is_none() {
                bail!("{}", i18n::board_card_target_not_found(locale));
            }
        }
        CardRef::Note(text) if text.trim().is_empty() => {
            bail!("note text can't be empty");
        }
        CardRef::Note(_) => {}
    }
    let (mut board, etag) = board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let Some(column) = board.columns.get_mut(column_index) else {
        bail!("{}", i18n::board_column_not_found(locale));
    };
    let id = board.next_card_id;
    column.cards.push(BoardCard { id, card });
    board.next_card_id += 1;
    board::put_board(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &board,
        etag.as_deref(),
    )
    .await
}

fn locate_card(board: &Board, card_id: u64) -> Option<(usize, usize)> {
    for (ci, column) in board.columns.iter().enumerate() {
        if let Some(pi) = column.cards.iter().position(|c| c.id == card_id) {
            return Some((ci, pi));
        }
    }
    None
}

/// Moves `card_id` to the end of `to_column_index`'s list — always appends,
/// never a specific position.
pub async fn move_card_to_column(
    state: &TrackerState,
    target: &TrackerTarget,
    card_id: u64,
    to_column_index: usize,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut board, etag) = board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let Some((from_column, from_pos)) = locate_card(&board, card_id) else {
        bail!("{}", i18n::board_card_not_found(locale));
    };
    if board.columns.get(to_column_index).is_none() {
        bail!("{}", i18n::board_column_not_found(locale));
    }
    let card = board.columns[from_column].cards.remove(from_pos);
    board.columns[to_column_index].cards.push(card);
    board::put_board(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &board,
        etag.as_deref(),
    )
    .await
}

/// Swaps `card_id` with its neighbor one position earlier (`up`) or later
/// (`down`) within its own column — a no-op (not an error) at either end.
async fn swap_within_column(
    state: &TrackerState,
    target: &TrackerTarget,
    card_id: u64,
    delta: isize,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut board, etag) = board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let Some((column_index, pos)) = locate_card(&board, card_id) else {
        bail!("{}", i18n::board_card_not_found(locale));
    };
    let cards = &mut board.columns[column_index].cards;
    let Some(new_pos) = pos.checked_add_signed(delta).filter(|&p| p < cards.len()) else {
        return Ok(Cas::Ok);
    };
    cards.swap(pos, new_pos);
    board::put_board(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &board,
        etag.as_deref(),
    )
    .await
}

pub async fn move_card_up(
    state: &TrackerState,
    target: &TrackerTarget,
    card_id: u64,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    swap_within_column(state, target, card_id, -1, locale).await
}

pub async fn move_card_down(
    state: &TrackerState,
    target: &TrackerTarget,
    card_id: u64,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    swap_within_column(state, target, card_id, 1, locale).await
}

pub async fn remove_card(
    state: &TrackerState,
    target: &TrackerTarget,
    card_id: u64,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut board, etag) = board::get_board(state.backend.as_ref(), &state.cipher, &target.uid).await?;
    let Some((column_index, pos)) = locate_card(&board, card_id) else {
        bail!("{}", i18n::board_card_not_found(locale));
    };
    board.columns[column_index].cards.remove(pos);
    board::put_board(
        state.backend.as_ref(),
        &state.cipher,
        &target.uid,
        &board,
        etag.as_deref(),
    )
    .await
}
