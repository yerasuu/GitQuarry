//! `webui::repo::components`'s page copy — shared table headers reused by
//! summary/tree/log/refs/activity.

use super::super::super::{Locale, msg};

msg!(name_column() { Locale::Es => "nombre", Locale::En => "name" });
msg!(commit_column() { Locale::Es => "commit", Locale::En => "commit" });
msg!(message_column() { Locale::Es => "mensaje", Locale::En => "message" });
msg!(date_column() { Locale::Es => "fecha", Locale::En => "date" });
msg!(author_column() { Locale::Es => "autor", Locale::En => "author" });
msg!(changes_column() { Locale::Es => "cambios", Locale::En => "changes" });
msg!(none_hint() { Locale::Es => "(ninguno)", Locale::En => "(none)" });
msg!(edit_comment_button() { Locale::Es => "editar", Locale::En => "edit" });
msg!(delete_comment_button() { Locale::Es => "borrar", Locale::En => "delete" });
msg!(save_comment_button() { Locale::Es => "guardar", Locale::En => "save" });
msg!(delete_comment_confirm() {
    Locale::Es => "¿borrar este comentario? no se puede deshacer.",
    Locale::En => "delete this comment? this cannot be undone.",
});

// `webui::diff_view`'s unified/split toggle above a diff.
msg!(diff_view_unified_button() { Locale::Es => "unificado", Locale::En => "unified" });
msg!(diff_view_split_button() { Locale::Es => "lado a lado", Locale::En => "side by side" });
