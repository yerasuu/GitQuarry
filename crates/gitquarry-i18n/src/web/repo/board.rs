//! `webui::repo::board`'s page copy.

use super::super::super::{Locale, msg};

msg!(board_heading() { Locale::Es => "tablero", Locale::En => "board" });
msg!(no_columns_hint() { Locale::Es => "(sin columnas todavia)", Locale::En => "(no columns yet)" });
msg!(new_column_heading() { Locale::Es => "nueva columna", Locale::En => "new column" });
msg!(new_column_placeholder() { Locale::Es => "nombre de la columna", Locale::En => "column name" });
msg!(add_column_button() { Locale::Es => "agregar columna", Locale::En => "add column" });
msg!(remove_column_button() { Locale::Es => "borrar columna", Locale::En => "delete column" });

msg!(add_card_summary() { Locale::Es => "agregar tarjeta", Locale::En => "add card" });
msg!(card_kind_label() { Locale::Es => "tipo", Locale::En => "kind" });
msg!(card_kind_note() { Locale::Es => "nota", Locale::En => "note" });
msg!(card_kind_issue() { Locale::Es => "issue", Locale::En => "issue" });
msg!(card_kind_merge_request() { Locale::Es => "merge request", Locale::En => "merge request" });
msg!(card_value_label() { Locale::Es => "valor", Locale::En => "value" });
msg!(card_value_placeholder() {
    Locale::Es => "numero de issue/PR, o texto de la nota",
    Locale::En => "issue/PR number, or the note's text",
});
msg!(add_card_button() { Locale::Es => "agregar tarjeta", Locale::En => "add card" });
msg!(remove_card_button() { Locale::Es => "quitar", Locale::En => "remove" });
msg!(move_button() { Locale::Es => "mover", Locale::En => "move" });
msg!(move_to_column_option(target: usize) {
    Locale::Es => "columna {target}",
    Locale::En => "column {target}",
});

msg!(closed_badge() { Locale::Es => "cerrado", Locale::En => "closed" });
msg!(merged_badge() { Locale::Es => "mergeado", Locale::En => "merged" });
msg!(broken_reference_hint() { Locale::Es => "(referencia rota)", Locale::En => "(broken reference)" });
