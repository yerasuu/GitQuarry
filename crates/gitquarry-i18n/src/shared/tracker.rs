//! Issue/board/milestone messages — `gitquarry-webui`'s own copies for
//! rendering `gitquarry-tracker`'s errors (`ctx.tracker`'s loopback API has
//! no `Locale` of its own to render with). `gitquarry-tracker` keeps a
//! second, independent copy of the overlapping ones in its own crate
//! (`gitquarry_tracker::i18n`) rather than depending on this crate just for
//! them — see that module's own doc comment
//! (`docs/tracker-binary-split-concept.md`).

use super::super::{Locale, msg};

msg!(
    /// `[tracker]` compiled in (`feature = "tracker"`) but not configured
    /// for this deploy — `ctx.tracker` is `None`. Shared across the
    /// issues/board/milestones views rather than tripled per-view.
    tracker_not_configured() {
        Locale::Es => "el tracker no esta configurado en este servidor",
        Locale::En => "tracker isn't configured on this server",
    }
);

msg!(
    /// No `IssueMeta` exists for `number` in this repo — same shape as
    /// `merge_request::merge_request_not_found`, confirmed duplicated
    /// across `domain::issue::{get, transition, add_comment, edit}`.
    issue_not_found(number: u64) {
        Locale::Es => "no existe el issue #{number}",
        Locale::En => "no issue #{number} exists",
    }
);

msg!(
    /// `domain::issue::transition`: the issue's current status doesn't
    /// allow the requested move (e.g. reopening an already-open issue).
    issue_wrong_status() {
        Locale::Es => "este issue no esta en un estado valido para esa accion",
        Locale::En => "this issue isn't in a valid status for that action",
    }
);

msg!(
    /// No `MilestoneMeta` exists for `number` in this repo — same shape as
    /// `issue_not_found`, confirmed duplicated across
    /// `domain::milestone::{get, transition}` and `domain::issue::edit`'s
    /// own milestone-number check.
    milestone_not_found(number: u64) {
        Locale::Es => "no existe el milestone #{number}",
        Locale::En => "no milestone #{number} exists",
    }
);

msg!(
    /// `domain::milestone::transition`: the milestone's current status
    /// doesn't allow the requested move.
    milestone_wrong_status() {
        Locale::Es => "este milestone no esta en un estado valido para esa accion",
        Locale::En => "this milestone isn't in a valid status for that action",
    }
);

msg!(
    /// `domain::board::{rename_column, remove_column, add_card, move_card}`:
    /// the column index doesn't name an existing column — most likely a
    /// stale form submitted after someone else already removed it.
    board_column_not_found() {
        Locale::Es => "esa columna ya no existe",
        Locale::En => "that column no longer exists",
    }
);

msg!(
    /// `domain::board::remove_column`: refuses to drop a column that still
    /// has cards on it — a `Note` card's text has no copy anywhere else, so
    /// silently discarding it on column removal would be real data loss,
    /// not just an organizational change. Move or remove every card first.
    board_column_not_empty() {
        Locale::Es => "mueve o borra las tarjetas de esta columna antes de borrarla",
        Locale::En => "move or remove this column's cards before deleting it",
    }
);

msg!(
    /// `domain::board::{move_card, remove_card}`: the card id doesn't name
    /// an existing card — most likely a stale form submitted after someone
    /// else already moved/removed it.
    board_card_not_found() {
        Locale::Es => "esa tarjeta ya no existe",
        Locale::En => "that card no longer exists",
    }
);

msg!(
    /// `domain::board::add_card`: an `Issue`/`MergeRequest` card must point
    /// at a real, existing issue/MR in this same repo — never a dangling
    /// number a card would otherwise render as a broken link forever.
    board_card_target_not_found() {
        Locale::Es => "ese issue o merge request no existe en este repo",
        Locale::En => "that issue or merge request doesn't exist in this repo",
    }
);
