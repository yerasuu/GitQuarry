//! A single kanban board per repo — one JSON blob (a handful of columns,
//! at most a few dozen cards) rather than a per-card collection like
//! [`super::issues`]/[`super::milestones`]: the whole board is always read
//! and rendered together, so one CAS-guarded read/write of the lot is
//! simpler than coordinating many independent card keys for no real
//! benefit — a board just isn't expected to grow past what a single JSON
//! object comfortably holds.

use crate::storage::REPOS_PREFIX;
use anyhow::Result;
use gitquarry_wire::Uid;
use s3db::Collection;
use s3db::{Backend, Cas};
use s3db_crypto::Cipher;
use serde::{Deserialize, Serialize};

/// What a card points at. `Note` carries its own text since it has no
/// backing issue/MR to point at — unlike the other two variants, deleting a
/// `Note` card is the only copy of that text gone for good, which is why
/// [`crate::domain::board::remove_column`] refuses to drop a non-empty
/// column instead of silently discarding whatever notes were still on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardRef {
    Issue(u64),
    MergeRequest(u64),
    Note(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardCard {
    /// Stable per-board id (see [`Board::next_card_id`]) — what
    /// [`crate::domain::board::move_card_up`]/`move_card_down`/`remove_card`
    /// address a card by, so a card keeps its identity across reorders/
    /// column moves instead of being addressed by a position that shifts
    /// under it.
    pub id: u64,
    pub card: CardRef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardColumn {
    pub name: String,
    pub cards: Vec<BoardCard>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Board {
    pub columns: Vec<BoardColumn>,
    /// Next id [`crate::domain::board::add_card`] mints — monotonic for the
    /// life of the board, never reused even after a card is removed, same
    /// "never reused" guarantee repo/issue/PR uids already give.
    #[serde(default)]
    pub next_card_id: u64,
}

/// Reserved key holding the whole board — never a valid card id's own key
/// (cards aren't individually keyed at all in this collection), just how
/// `Collection` addresses a single blob the same way `milestones::COUNTER_KEY`
/// addresses its own single counter object.
const BOARD_KEY: &str = "board";

fn board_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/board"))
}

pub async fn get_board(backend: &dyn Backend, cipher: &Cipher, uid: &Uid) -> Result<(Board, Option<String>)> {
    let collection = board_collection(uid);
    let (board, etag) = collection.get::<Board>(backend, cipher, BOARD_KEY).await?;
    Ok((board.unwrap_or_default(), etag))
}

pub async fn put_board(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    board: &Board,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    let collection = board_collection(uid);
    collection.put(backend, cipher, BOARD_KEY, board, expected_etag).await
}
