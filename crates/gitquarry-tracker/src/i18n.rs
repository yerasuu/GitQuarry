//! `gitquarry-tracker`'s own small error-message catalog — a dozen strings
//! its domain layer emits today, copied once from `gitquarry-i18n::shared`
//! rather than depending on that whole crate for a dozen messages
//! (`docs/tracker-binary-split-concept.md`'s own reasoning: depending on
//! `gitquarry-core`'s i18n just for this would reintroduce exactly the
//! dependency the split exists to sever). Same `msg!`-macro shape, En/Es,
//! same "no `_ =>` wildcard" convention.

use gitquarry_i18n::Locale;

gitquarry_i18n::msg!(
    issue_not_found(number: u64) {
        Locale::Es => "no existe el issue #{number}",
        Locale::En => "no issue #{number} exists",
    }
);

gitquarry_i18n::msg!(
    issue_wrong_status() {
        Locale::Es => "este issue no esta en un estado valido para esa accion",
        Locale::En => "this issue isn't in a valid status for that action",
    }
);

gitquarry_i18n::msg!(
    unknown_label(name: &str) {
        Locale::Es => "no existe la etiqueta \"{name}\" en este repo",
        Locale::En => "no such label \"{name}\" on this repo",
    }
);

gitquarry_i18n::msg!(
    unknown_assignee(username: &str) {
        Locale::Es => "no existe el usuario \"{username}\"",
        Locale::En => "no such user \"{username}\"",
    }
);

gitquarry_i18n::msg!(
    milestone_not_found(number: u64) {
        Locale::Es => "no existe el milestone #{number}",
        Locale::En => "no milestone #{number} exists",
    }
);

gitquarry_i18n::msg!(
    milestone_wrong_status() {
        Locale::Es => "este milestone no esta en un estado valido para esa accion",
        Locale::En => "this milestone isn't in a valid status for that action",
    }
);

gitquarry_i18n::msg!(
    board_column_not_found() {
        Locale::Es => "esa columna ya no existe",
        Locale::En => "that column no longer exists",
    }
);

gitquarry_i18n::msg!(
    board_column_not_empty() {
        Locale::Es => "mueve o borra las tarjetas de esta columna antes de borrarla",
        Locale::En => "move or remove this column's cards before deleting it",
    }
);

gitquarry_i18n::msg!(
    board_card_not_found() {
        Locale::Es => "esa tarjeta ya no existe",
        Locale::En => "that card no longer exists",
    }
);

gitquarry_i18n::msg!(
    board_card_target_not_found() {
        Locale::Es => "ese issue o pull request no existe en este repo",
        Locale::En => "that issue or pull request doesn't exist in this repo",
    }
);

gitquarry_i18n::msg!(
    not_comment_author() {
        Locale::Es => "solo quien escribio el comentario puede editarlo o borrarlo",
        Locale::En => "only the comment's own author can edit or delete it",
    }
);
