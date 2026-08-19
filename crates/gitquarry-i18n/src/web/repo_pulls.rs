//! `webui::repo::pulls`'s page copy.

use super::super::{Locale, msg};

msg!(number_column() { Locale::Es => "#", Locale::En => "#" });
msg!(title_column() { Locale::Es => "titulo", Locale::En => "title" });
msg!(status_column() { Locale::Es => "estado", Locale::En => "status" });
msg!(author_column() { Locale::Es => "autor", Locale::En => "author" });
msg!(no_pulls_yet_hint() { Locale::Es => "(sin pull requests aun)", Locale::En => "(no pull requests yet)" });
msg!(no_matches_hint() { Locale::Es => "(sin resultados)", Locale::En => "(no matches)" });
msg!(search_pulls_placeholder() { Locale::Es => "buscar en titulo y descripcion", Locale::En => "search title and description" });
msg!(search_button() { Locale::Es => "buscar", Locale::En => "search" });
msg!(bulk_close_option() { Locale::Es => "cerrar", Locale::En => "close" });
msg!(bulk_reopen_option() { Locale::Es => "reabrir", Locale::En => "reopen" });
msg!(bulk_apply_button() { Locale::Es => "aplicar a los seleccionados", Locale::En => "apply to selected" });

msg!(open_status() { Locale::Es => "abierto", Locale::En => "open" });
msg!(closed_status() { Locale::Es => "cerrado", Locale::En => "closed" });
msg!(merged_status() { Locale::Es => "fusionado", Locale::En => "merged" });
msg!(all_status_filter() { Locale::Es => "todos", Locale::En => "all" });

msg!(new_pull_heading() { Locale::Es => "nuevo pull request", Locale::En => "new pull request" });
msg!(new_pull_button() { Locale::Es => "nuevo pull request", Locale::En => "new pull request" });
msg!(source_repo_label() { Locale::Es => "repo origen", Locale::En => "source repo" });
msg!(source_repo_placeholder() {
    Locale::Es => "vacio para este repo, o owner/name de tu fork",
    Locale::En => "blank for this repo, or owner/name of your fork",
});
msg!(source_branch_label() { Locale::Es => "rama origen", Locale::En => "source branch" });
msg!(target_branch_label() { Locale::Es => "rama destino", Locale::En => "target branch" });
msg!(title_label() { Locale::Es => "titulo", Locale::En => "title" });
msg!(description_label() { Locale::Es => "descripcion", Locale::En => "description" });
msg!(create_pull_button() { Locale::Es => "crear pull request", Locale::En => "create pull request" });

msg!(opened_by_prefix() { Locale::Es => "abierto por ", Locale::En => "opened by " });
msg!(opened_by_middle() { Locale::Es => " el ", Locale::En => " on " });
msg!(close_button() { Locale::Es => "cerrar", Locale::En => "close" });
msg!(reopen_button() { Locale::Es => "reabrir", Locale::En => "reopen" });
msg!(merge_button() { Locale::Es => "fusionar", Locale::En => "merge" });
msg!(merge_confirm() {
    Locale::Es => "fusionar este pull request? esta accion no se puede deshacer.",
    Locale::En => "merge this pull request? this cannot be undone.",
});
msg!(merge_strategy_label() { Locale::Es => "estrategia de fusion", Locale::En => "merge strategy" });
msg!(merge_strategy_merge_option() { Locale::Es => "fusion (merge commit)", Locale::En => "merge commit" });
msg!(merge_strategy_squash_option() { Locale::Es => "aplastar y fusionar (squash)", Locale::En => "squash and merge" });
msg!(merge_strategy_rebase_option() { Locale::Es => "rebasar y fusionar (rebase)", Locale::En => "rebase and merge" });
msg!(delete_branch_checkbox_label() { Locale::Es => "eliminar rama origen", Locale::En => "delete source branch" });
msg!(merged_by_prefix() { Locale::Es => "fusionado por ", Locale::En => "merged by " });
msg!(merged_by_middle() { Locale::Es => " el ", Locale::En => " on " });
msg!(merge_commit_prefix() { Locale::Es => "commit de fusion: ", Locale::En => "merge commit: " });
msg!(no_commits_hint() { Locale::Es => "sin commits nuevos", Locale::En => "no new commits" });

msg!(draft_badge() { Locale::Es => "borrador", Locale::En => "draft" });
msg!(mark_draft_button() { Locale::Es => "marcar como borrador", Locale::En => "mark as draft" });
msg!(mark_ready_button() { Locale::Es => "marcar como listo", Locale::En => "mark ready for review" });
msg!(edit_metadata_heading() { Locale::Es => "editar", Locale::En => "edit" });
msg!(labels_label() { Locale::Es => "etiquetas", Locale::En => "labels" });
msg!(labels_placeholder() { Locale::Es => "separadas por comas", Locale::En => "comma-separated" });
msg!(assignees_label() { Locale::Es => "asignados", Locale::En => "assignees" });
msg!(assignees_placeholder() { Locale::Es => "usuarios separados por comas", Locale::En => "comma-separated usernames" });
msg!(reviewers_label() { Locale::Es => "revisores solicitados", Locale::En => "requested reviewers" });
msg!(reviewers_placeholder() { Locale::Es => "usuarios separados por comas", Locale::En => "comma-separated usernames" });
msg!(save_button() { Locale::Es => "guardar", Locale::En => "save" });

msg!(comments_heading() { Locale::Es => "comentarios", Locale::En => "comments" });
msg!(no_comments_hint() { Locale::Es => "(sin comentarios aun)", Locale::En => "(no comments yet)" });
msg!(comment_button() { Locale::Es => "comentar", Locale::En => "comment" });

msg!(comment_on_line_marker() { Locale::Es => "[+]", Locale::En => "[+]" });
msg!(line_comment_prefix() { Locale::Es => "en ", Locale::En => "on " });

msg!(reviews_heading() { Locale::Es => "revisiones", Locale::En => "reviews" });
msg!(no_reviews_hint() { Locale::Es => "(sin revisiones aun)", Locale::En => "(no reviews yet)" });
msg!(approved_verdict() { Locale::Es => "aprobo", Locale::En => "approved" });
msg!(changes_requested_verdict() { Locale::Es => "pidio cambios", Locale::En => "requested changes" });
msg!(commented_verdict() { Locale::Es => "comento", Locale::En => "commented" });
msg!(approved_by_prefix() { Locale::Es => "aprobado por: ", Locale::En => "approved by: " });
msg!(changes_requested_by_prefix() {
    Locale::Es => "cambios pedidos por: ",
    Locale::En => "changes requested by: ",
});
msg!(review_verdict_label() { Locale::Es => "veredicto", Locale::En => "verdict" });
msg!(submit_review_button() { Locale::Es => "enviar revision", Locale::En => "submit review" });
msg!(own_pull_request_review_hint() {
    Locale::Es => "es tu pull request: solo podes dejar una revision neutral",
    Locale::En => "this is your own pull request: you can only leave a neutral review",
});

// Conflict notice on an open PR's page — the file list used to exist only
// as a `400` thrown by the merge handler.
msg!(conflicts_heading() { Locale::Es => "conflictos con la rama destino", Locale::En => "conflicts with the target branch" });
msg!(conflicts_intro(target: &str) {
    Locale::Es => "Esta rama no se puede fusionar automaticamente en {target}. Los siguientes ficheros cambian en ambos lados:",
    Locale::En => "This branch can't be merged into {target} automatically. These files changed on both sides:",
});
msg!(conflicts_howto_summary() {
    Locale::Es => "como resolverlo en local",
    Locale::En => "how to resolve it locally",
});
