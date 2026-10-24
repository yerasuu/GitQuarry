//! Merge-request messages — `domain::merge_request` and the review/comment
//! checks it shares no other domain needs.

use super::super::{Locale, msg};

msg!(
    /// No `MergeRequestMeta` exists for `number` in this repo — confirmed
    /// duplicated across `domain::merge_request::{get, transition, add_comment}`.
    merge_request_not_found(number: u64) {
        Locale::Es => "no existe el merge request #{number}",
        Locale::En => "no merge request #{number} exists",
    }
);

msg!(
    /// `domain::merge_request::merge` refusing a fast-forward: the target
    /// branch moved on, so no pointer move expresses the merge any more.
    merge_request_not_fast_forwardable(branch: &str) {
        Locale::Es => "no se puede avanzar {branch} sin crear un commit: la rama ha divergido, usa merge, squash o rebase",
        Locale::En => "can't fast-forward {branch} without creating a commit: the branch has diverged, use merge, squash or rebase",
    }
);

msg!(
    /// `domain::merge_request::transition`: the MR's current status doesn't
    /// allow the requested move (e.g. reopening a merged MR, closing an
    /// already-closed one).
    merge_request_wrong_status() {
        Locale::Es => "este merge request no esta en un estado valido para esa accion",
        Locale::En => "this merge request isn't in a valid status for that action",
    }
);

msg!(
    /// `domain::merge_request::create`: a cross-repo MR's `source_repo` isn't
    /// actually a fork of the target repo (`RepoConfig::forked_from` doesn't
    /// point back at it) — cross-repo MRs are only allowed fork -> upstream,
    /// never between two unrelated repos.
    source_repo_not_a_fork() {
        Locale::Es => "el repo origen no es un fork de este repo",
        Locale::En => "the source repo isn't a fork of this repo",
    }
);

msg!(
    /// `domain::merge_request::merge`: the MR is still a draft — flip it
    /// with `domain::merge_request::mark_ready` before merging.
    merge_request_is_draft() {
        Locale::Es => "este merge request todavia es un borrador",
        Locale::En => "this merge request is still a draft",
    }
);

msg!(
    /// `domain::merge_request::merge`: at least one reviewer's *latest*
    /// verdict (`domain::merge_request::review_summary`) is
    /// `ChangesRequested` — merging is blocked until they either submit a
    /// new verdict or the requested changes land.
    merge_request_changes_requested_pending() {
        Locale::Es => "hay cambios pedidos sin resolver en este merge request",
        Locale::En => "this merge request has unresolved requested changes",
    }
);

msg!(
    /// `domain::merge_request::merge`, via `missing_required_reviewers`:
    /// `names` is a comma-separated list of usernames who still haven't
    /// approved but are required to, either by `RepoConfig::required_reviewers`
    /// or by `CODEOWNERS` for a path this MR touches.
    merge_request_missing_required_reviewers(names: &str) {
        Locale::Es => "falta la aprobacion de: {names}",
        Locale::En => "still missing approval from: {names}",
    }
);

msg!(
    /// `domain::merge_request::edit_metadata`: a submitted reviewer
    /// username doesn't resolve to a real user — same check as
    /// `unknown_assignee`, worded for the reviewers field instead.
    unknown_reviewer(username: &str) {
        Locale::Es => "no existe el usuario \"{username}\"",
        Locale::En => "no such user \"{username}\"",
    }
);

msg!(
    /// `domain::merge_request::add_line_comment`: the submitted anchor has no
    /// path, or a line number of 0 (line numbers are 1-based) — a hand-rolled
    /// POST, since every form that offers a line comment fills both in.
    invalid_comment_anchor() {
        Locale::Es => "el comentario no apunta a una linea valida del diff",
        Locale::En => "the comment doesn't point at a valid diff line",
    }
);

msg!(
    /// `domain::merge_request::add_review`: an MR's own author submitting an
    /// approval or a change request on it. Reviewing your own work isn't a
    /// signal, so only the neutral `Commented` verdict is allowed there.
    cannot_review_own_merge_request() {
        Locale::Es => "no podes aprobar ni pedir cambios en tu propio merge request",
        Locale::En => "you can't approve or request changes on your own merge request",
    }
);

msg!(
    /// `domain::merge_request::add_review`: the source branch moved after
    /// the page the reviewer submitted from was rendered, so what they
    /// reviewed is no longer what would be merged.
    review_of_outdated_head() {
        Locale::Es => "la rama origen cambio desde que cargaste la pagina: recarga y revisa el codigo actual",
        Locale::En => "the source branch changed since you loaded the page: reload and review the current code",
    }
);

msg!(
    /// Editing a comment is strictly author-only, no override at all.
    /// Deleting one is author-only too *unless* the actor is `Developer`+ on
    /// the repo (`domain::issue::delete_comment`'s own `is_moderator`
    /// param) — moderating someone else's abusive comment away is different
    /// from silently rewriting it. Shared between
    /// `domain::issue::{edit_comment, delete_comment}` and
    /// `domain::merge_request`'s own equivalents.
    not_comment_author() {
        Locale::Es => "solo quien escribio el comentario puede editarlo o borrarlo",
        Locale::En => "only the comment's own author can edit or delete it",
    }
);

msg!(
    /// An MR was submitted with no title and its source branch's tip commit
    /// has no subject either, so there is nothing to name it with — see
    /// `gitquarry_core::domain::merge_request::resolve_title_and_description`.
    merge_request_needs_a_title() {
        Locale::Es => "el merge request necesita un titulo, y el ultimo commit de la rama no tiene asunto",
        Locale::En => "the merge request needs a title, and the branch's last commit has no subject",
    }
);
