//! `webui::repo::issues`'s page copy.

use super::super::super::{Locale, msg};

msg!(number_column() { Locale::Es => "#", Locale::En => "#" });
msg!(title_column() { Locale::Es => "titulo", Locale::En => "title" });
msg!(status_column() { Locale::Es => "estado", Locale::En => "status" });
msg!(author_column() { Locale::Es => "autor", Locale::En => "author" });
msg!(no_issues_yet_hint() { Locale::Es => "(sin issues aun)", Locale::En => "(no issues yet)" });
msg!(no_matches_hint() { Locale::Es => "(sin resultados)", Locale::En => "(no matches)" });
msg!(search_issues_placeholder() { Locale::Es => "buscar en titulo y descripcion", Locale::En => "search title and body" });
msg!(search_button() { Locale::Es => "buscar", Locale::En => "search" });
msg!(bulk_close_option() { Locale::Es => "cerrar", Locale::En => "close" });
msg!(bulk_reopen_option() { Locale::Es => "reabrir", Locale::En => "reopen" });
msg!(bulk_apply_button() { Locale::Es => "aplicar a los seleccionados", Locale::En => "apply to selected" });

msg!(open_status() { Locale::Es => "abierto", Locale::En => "open" });
msg!(closed_status() { Locale::Es => "cerrado", Locale::En => "closed" });
msg!(all_status_filter() { Locale::Es => "todos", Locale::En => "all" });

msg!(new_issue_heading() { Locale::Es => "nuevo issue", Locale::En => "new issue" });
msg!(new_issue_button() { Locale::Es => "nuevo issue", Locale::En => "new issue" });
msg!(title_label() { Locale::Es => "titulo", Locale::En => "title" });
msg!(body_label() { Locale::Es => "descripcion", Locale::En => "description" });
msg!(create_issue_button() { Locale::Es => "crear issue", Locale::En => "create issue" });

msg!(opened_by_prefix() { Locale::Es => "abierto por ", Locale::En => "opened by " });
msg!(opened_by_middle() { Locale::Es => " el ", Locale::En => " on " });
msg!(close_button() { Locale::Es => "cerrar", Locale::En => "close" });
msg!(reopen_button() { Locale::Es => "reabrir", Locale::En => "reopen" });

msg!(edit_heading() { Locale::Es => "editar issue", Locale::En => "edit issue" });
msg!(labels_label() { Locale::Es => "etiquetas", Locale::En => "labels" });
msg!(labels_placeholder() { Locale::Es => "separadas por comas", Locale::En => "comma-separated" });
msg!(assignees_label() { Locale::Es => "asignados", Locale::En => "assignees" });
msg!(assignees_placeholder() { Locale::Es => "usuarios separados por comas", Locale::En => "comma-separated usernames" });
msg!(milestone_label() { Locale::Es => "milestone", Locale::En => "milestone" });
msg!(save_button() { Locale::Es => "guardar", Locale::En => "save" });

msg!(comments_heading() { Locale::Es => "comentarios", Locale::En => "comments" });
msg!(no_comments_hint() { Locale::Es => "(sin comentarios aun)", Locale::En => "(no comments yet)" });
msg!(comment_button() { Locale::Es => "comentar", Locale::En => "comment" });
msg!(edit_comment_button() { Locale::Es => "editar", Locale::En => "edit" });
msg!(delete_comment_button() { Locale::Es => "borrar", Locale::En => "delete" });
msg!(delete_comment_confirm() {
    Locale::Es => "borrar este comentario? esta accion no se puede deshacer.",
    Locale::En => "delete this comment? this cannot be undone.",
});
