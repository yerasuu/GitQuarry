use super::super::{Locale, msg};

// Reused across several of the page's sub-forms (default branch, protected
// branches/tags, topics, repo conf, the per-row permission grid) — all six
// submit buttons say exactly "save", so one message covers every call site
// instead of six near-identical entries (same dedup the groups.rs catalog
// already uses for its own "remove" button, reused across owner/team rows).
msg!(save_button() { Locale::Es => "guardar", Locale::En => "save" });
msg!(change_perm_confirm(username: &str) {
    Locale::Es => "¿cambiar el acceso de '{username}' a este repo?",
    Locale::En => "change '{username}''s access to this repo?",
});
msg!(save_general_confirm() {
    Locale::Es => "¿guardar los ajustes generales? incluye visibilidad y archivado.",
    Locale::En => "save general settings? this includes visibility and archived state.",
});
msg!(save_protected_branches_confirm() {
    Locale::Es => "¿guardar la lista de ramas protegidas? quitar una entrada le retira la proteccion.",
    Locale::En => "save the protected branches list? removing an entry unprotects it.",
});
msg!(save_protected_tags_confirm() {
    Locale::Es => "¿guardar la lista de tags protegidos? quitar una entrada le retira la proteccion.",
    Locale::En => "save the protected tags list? removing an entry unprotects it.",
});
msg!(save_required_reviewers_confirm() {
    Locale::Es => "¿guardar la lista de reviewers requeridos?",
    Locale::En => "save the required reviewers list?",
});
msg!(save_require_signed_commits_confirm() {
    Locale::Es => "¿guardar la lista de ramas con commits firmados requeridos? quitar una entrada le retira el requisito.",
    Locale::En => "save the signed-commits-required branch list? removing an entry drops the requirement.",
});
msg!(save_labels_confirm() {
    Locale::Es => "¿guardar las etiquetas? una etiqueta quitada de la lista se elimina tambien de los issues/MRs existentes.",
    Locale::En => "save labels? a label removed from the list is also removed from existing issues/MRs.",
});

// Both the delete-repo and move-repo forms end their confirmation prose the
// same way: "... Type the full name (<code>repo</code>) to confirm." — shared
// so the wording only has to agree in one place.
msg!(type_full_name_prefix() { Locale::Es => "Escribe el nombre completo (", Locale::En => "Type the full name (" });
msg!(to_confirm_suffix() { Locale::Es => ") para confirmar.", Locale::En => ") to confirm." });

msg!(restore_repo_label() { Locale::Es => "restaurar repo", Locale::En => "restore repo" });
msg!(deleted_repo_notice(days_left: u64) {
    Locale::Es => "Este repo fue borrado. Se purgará de forma irreversible en {days_left} día(s) si no lo restaurás antes.",
    Locale::En => "This repo was deleted. It will be permanently purged in {days_left} day(s) unless you restore it first.",
});

// Admin-only: skips the rest of the grace window instead of waiting for
// `mirror::deleted_repo_sweep`. Same displayed text for heading and button
// as `delete_repo_label` does for its own form.
msg!(force_purge_label() { Locale::Es => "eliminar ahora (admin)", Locale::En => "delete now (admin)" });
msg!(force_purge_prose() {
    Locale::Es => "Solo administradores: se salta el resto de la ventana de gracia y purga de inmediato, sin posibilidad de restaurar. ",
    Locale::En => "Admins only: skips the rest of the grace window and purges immediately, with no way to restore. ",
});

msg!(default_branch_heading() { Locale::Es => "rama por defecto", Locale::En => "default branch" });

msg!(protected_branches_heading() { Locale::Es => "ramas protegidas", Locale::En => "protected branches" });
msg!(protected_branches_prose(branches: &str) {
    Locale::Es => "Rechaza force-push y borrado. Solo un propietario (del repo o de su grupo propietario) puede pushear directamente. Ramas disponibles: {branches}.",
    Locale::En => "Rejects force-push and deletion. Only an owner (of the repo or its owning group) may push directly. Available branches: {branches}.",
});
msg!(protected_branches_placeholder() { Locale::Es => "main, release/1.0", Locale::En => "main, release/1.0" });

msg!(protected_tags_heading() { Locale::Es => "tags protegidos", Locale::En => "protected tags" });
msg!(protected_tags_prose(tags: &str) {
    Locale::Es => "Rechaza mover o borrar, sea fast-forward o no. Solo un propietario (del repo o de su grupo propietario) puede pushear directamente. Tags disponibles: {tags}.",
    Locale::En => "Rejects moving or deleting, fast-forward or not. Only an owner (of the repo or its owning group) may push directly. Available tags: {tags}.",
});
msg!(protected_tags_placeholder() { Locale::Es => "v1.0, v2.0", Locale::En => "v1.0, v2.0" });

msg!(require_signed_commits_heading() { Locale::Es => "commits firmados", Locale::En => "signed commits" });
msg!(require_signed_commits_prose(branches: &str) {
    Locale::Es => "Rechaza cualquier commit que no venga firmado con una clave GPG registrada en una cuenta con permiso de push (no alcanza con cualquier firma valida). Ramas disponibles: {branches}.",
    Locale::En => "Rejects any commit that doesn't carry a GPG signature from a key registered to an account with push permission (any valid signature isn't enough on its own). Available branches: {branches}.",
});
msg!(require_signed_commits_placeholder() { Locale::Es => "main, release/1.0", Locale::En => "main, release/1.0" });

msg!(topics_heading() { Locale::Es => "temas", Locale::En => "topics" });
msg!(topics_prose() {
    Locale::Es => "Etiquetas libres para categorizar/buscar — normalizadas a minusculas, solo letras/numeros/guion. Cada una enlaza a la busqueda del indice.",
    Locale::En => "Free-form labels for categorizing/searching — normalized to lowercase, letters/numbers/hyphen only. Each one links to the index search.",
});
msg!(topics_placeholder() { Locale::Es => "rust, cli, git", Locale::En => "rust, cli, git" });

msg!(required_reviewers_heading() { Locale::Es => "reviewers requeridos", Locale::En => "required reviewers" });
msg!(required_reviewers_prose() {
    Locale::Es => "Deben aprobar antes de poder mergear un MR, ademas de lo que pida un CODEOWNERS en la raiz del repo para los archivos que toque.",
    Locale::En => "Must approve before an MR can merge, on top of whatever a CODEOWNERS file at the repo root additionally requires for the files it touches.",
});
msg!(required_reviewers_placeholder() { Locale::Es => "alice, bob", Locale::En => "alice, bob" });

msg!(labels_heading() { Locale::Es => "etiquetas", Locale::En => "labels" });
msg!(labels_prose() {
    Locale::Es => "Vocabulario de etiquetas asignables a issues y merge requests de este repo, formato nombre:#color separado por comas.",
    Locale::En => "This repo's label vocabulary, assignable to issues and merge requests, name:#color pairs, comma-separated.",
});
msg!(labels_placeholder() { Locale::Es => "bug:#d73a4a, mejora:#a2eeef", Locale::En => "bug:#d73a4a, enhancement:#a2eeef" });

msg!(lfs_limit_heading() { Locale::Es => "limite de tamano (LFS)", Locale::En => "size limit (LFS)" });
msg!(lfs_limit_prose() {
    Locale::Es => "Rechaza un push que introduzca un blob por encima de este tamano y no convertido a puntero LFS. Vacio: sin limite.",
    Locale::En => "Rejects a push that introduces a blob over this size and not converted to an LFS pointer. Empty: no limit.",
});
msg!(lfs_limit_placeholder() { Locale::Es => "50M (numero + B/K/M/G)", Locale::En => "50M (number + B/K/M/G)" });

msg!(repo_conf_heading() { Locale::Es => "ajustes del repo", Locale::En => "repo settings" });
// Prose is split around the `<code>repo/.conf/config.json</code>` element —
// the path itself stays untranslated (a literal S3 key, not language-bound).
msg!(repo_conf_prose_prefix() { Locale::Es => "Se guarda en ", Locale::En => "Stored in " });
msg!(repo_conf_prose_suffix() {
    Locale::Es => " dentro del bucket. Privado: oculto y no servido por git:// ni la interfaz web (404); ssh:// no cambia, ya requiere una clave autorizada por repo (ver abajo).",
    Locale::En => " inside the bucket. Private: hidden and not served over git:// and the web UI (404); ssh:// doesn't change, it already requires a key authorized per repo (see below).",
});
msg!(field_label_description() { Locale::Es => "descripcion", Locale::En => "description" });
msg!(field_label_owner() { Locale::Es => "propietario", Locale::En => "owner" });
msg!(field_label_visibility() { Locale::Es => "visibilidad", Locale::En => "visibility" });
msg!(field_label_archived() { Locale::Es => "archivado", Locale::En => "archived" });
msg!(owner_option_none() { Locale::Es => "(ninguno)", Locale::En => "(none)" });
msg!(owner_derived_from_namespace_hint() {
    Locale::Es => "(segun el namespace del repo; para cambiarlo, mueve el repo mas abajo)",
    Locale::En => "(from the repo's own namespace; to change it, move the repo below)",
});
msg!(visibility_public_option() { Locale::Es => "publico", Locale::En => "public" });
msg!(visibility_limited_option() {
    Locale::Es => "limitado (solo usuarios con sesion iniciada)",
    Locale::En => "limited (logged-in users only)",
});
msg!(visibility_private_option() { Locale::Es => "privado", Locale::En => "private" });
msg!(archived_checkbox_hint() {
    Locale::Es => "rechaza pushes, se mantiene visible/clonable",
    Locale::En => "rejects pushes, stays visible/clonable",
});
msg!(field_label_is_template() { Locale::Es => "plantilla", Locale::En => "template" });
msg!(is_template_checkbox_hint() {
    Locale::Es => "otros pueden crear repos nuevos a partir de este",
    Locale::En => "others can create new repos from this one",
});

// Used for both the heading and the button — same displayed text either way.
msg!(delete_repo_label() { Locale::Es => "eliminar repo", Locale::En => "delete repo" });
msg!(delete_repo_prose_intro() { Locale::Es => "Irreversible: elimina ", Locale::En => "Irreversible: deletes " });
msg!(delete_repo_prose_pack_note() {
    Locale::Es => ", y cada pack de este repo en el bucket. ",
    Locale::En => ", and every pack of this repo in the bucket. ",
});

msg!(move_repo_heading() { Locale::Es => "mover repo", Locale::En => "move repo" });
msg!(move_repo_prose_intro() {
    Locale::Es => "Cambia el nombre/namespace publico del repo. El almacenamiento interno no se mueve (es solo un cambio de metadato, ningun objeto se recopia), pero cada URL de clonado de la ruta actual deja de resolver de inmediato. ",
    Locale::En => "Changes the repo's public name/namespace. The internal storage doesn't move (it's just a metadata change, no object gets re-copied), but every clone URL of the current path stops resolving immediately. ",
});
msg!(new_namespace_label() { Locale::Es => "nuevo namespace", Locale::En => "new namespace" });
msg!(new_name_label() { Locale::Es => "nuevo nombre", Locale::En => "new name" });
msg!(new_name_placeholder() { Locale::Es => "proyecto", Locale::En => "project" });
msg!(new_name_hint() {
    Locale::Es => "cualquier texto vale; la url se genera a partir de este nombre.",
    Locale::En => "any text works; the url is generated from this name."
});
msg!(confirm_label() { Locale::Es => "confirmar", Locale::En => "confirm" });
msg!(move_button() { Locale::Es => "mover", Locale::En => "move" });

msg!(perm_grid_heading() { Locale::Es => "permisos ssh:// por usuario", Locale::En => "ssh:// permissions by user" });
// Prose is split around two `<code>` elements (`*`, `visibility`) and the
// `/_users` link text.
msg!(perm_grid_prose_1() {
    Locale::Es => "Sin marcar (\"none\"): recae en el permiso por defecto (",
    Locale::En => "Unchecked (\"none\"): falls back to that user's ",
});
msg!(perm_grid_prose_2() {
    Locale::Es => ") de ese usuario si tiene uno (editable en ",
    Locale::En => " default if they have one (editable in ",
});
msg!(perm_grid_prose_3() {
    Locale::Es => "), si no, sin acceso. git:// y la web no tienen identidad — esto no las gobierna, solo ",
    Locale::En => "), otherwise no access. git:// and the web don't have an identity — this doesn't govern them, only ",
});
msg!(perm_grid_prose_4() { Locale::Es => " arriba.", Locale::En => " above." });
msg!(no_explicit_permissions_hint() {
    Locale::Es => "(sin permisos explicitos en este repo todavia)",
    Locale::En => "(no explicit permissions on this repo yet)",
});
msg!(table_header_user() { Locale::Es => "usuario", Locale::En => "user" });
msg!(table_header_access() { Locale::Es => "acceso", Locale::En => "access" });
msg!(radio_none_label() { Locale::Es => "ninguno", Locale::En => "none" });
msg!(radio_guest_label() { Locale::Es => "invitado", Locale::En => "guest" });
msg!(radio_reporter_label() { Locale::Es => "lector", Locale::En => "reporter" });
msg!(radio_developer_label() { Locale::Es => "desarrollador", Locale::En => "developer" });
msg!(radio_maintainer_label() { Locale::Es => "mantenedor", Locale::En => "maintainer" });
msg!(radio_owner_label() { Locale::Es => "propietario", Locale::En => "owner" });
msg!(add_access_heading() { Locale::Es => "agregar acceso", Locale::En => "add access" });
msg!(username_placeholder() { Locale::Es => "usuario", Locale::En => "username" });
msg!(add_button() { Locale::Es => "agregar", Locale::En => "add" });
msg!(permissions_heading() { Locale::Es => "permisos por usuario", Locale::En => "per-user permissions" });

msg!(mirrors_heading() { Locale::Es => "espejos externos", Locale::En => "external mirrors" });
msg!(mirrors_prose() {
    Locale::Es => "Sincroniza refs (ramas/tags) con un repo en GitHub, GitLab o Codeberg via git plano — sin API de proveedor, solo push/pull por https con un token embebido en la URL. Barrido periodico, mas \"sincronizar ahora\" por mirror.",
    Locale::En => "Syncs refs (branches/tags) with a repo on GitHub, GitLab, or Codeberg over plain git — no provider API, just push/pull over https with a token embedded in the URL. Periodic sweep, plus a per-mirror \"sync now\".",
});
msg!(no_mirrors_hint() { Locale::Es => "(sin espejos configurados todavia)", Locale::En => "(no mirrors configured yet)" });
msg!(mirror_table_header_label() { Locale::Es => "etiqueta", Locale::En => "label" });
msg!(mirror_table_header_provider() { Locale::Es => "proveedor", Locale::En => "provider" });
msg!(mirror_table_header_direction() { Locale::Es => "direccion", Locale::En => "direction" });
msg!(mirror_table_header_status() { Locale::Es => "estado por ref", Locale::En => "status by ref" });
msg!(mirror_table_header_actions() { Locale::Es => "acciones", Locale::En => "actions" });
msg!(mirror_field_label_url() { Locale::Es => "url remota", Locale::En => "remote url" });
msg!(mirror_url_placeholder() { Locale::Es => "https://github.com/duenio/repo.git", Locale::En => "https://github.com/owner/repo.git" });
msg!(mirror_field_label_token() { Locale::Es => "token", Locale::En => "token" });
msg!(mirror_token_placeholder_new() { Locale::Es => "token de acceso personal", Locale::En => "personal access token" });
msg!(mirror_token_placeholder_unchanged() {
    Locale::Es => "sin cambios (dejar en blanco)",
    Locale::En => "unchanged (leave blank)",
});
msg!(mirror_field_label_label() { Locale::Es => "etiqueta", Locale::En => "label" });
msg!(mirror_label_placeholder() { Locale::Es => "backup en github", Locale::En => "github backup" });
msg!(mirror_field_label_enabled() { Locale::Es => "habilitado", Locale::En => "enabled" });
msg!(direction_push_only_option() { Locale::Es => "solo push", Locale::En => "push only" });
msg!(direction_pull_only_option() { Locale::Es => "solo pull", Locale::En => "pull only" });
msg!(direction_bidirectional_option() { Locale::Es => "bidireccional", Locale::En => "bidirectional" });
msg!(direction_push_only_label() { Locale::Es => "push", Locale::En => "push" });
msg!(direction_pull_only_label() { Locale::Es => "pull", Locale::En => "pull" });
msg!(direction_bidirectional_label() { Locale::Es => "push+pull", Locale::En => "push+pull" });
msg!(ref_status_in_sync() { Locale::Es => "al dia", Locale::En => "in sync" });
msg!(ref_status_diverged() { Locale::Es => "divergido", Locale::En => "diverged" });
msg!(ref_status_error() { Locale::Es => "error", Locale::En => "error" });
msg!(mirror_last_error() { Locale::Es => "sync fallido", Locale::En => "sync failed" });
msg!(add_mirror_heading() { Locale::Es => "agregar espejo", Locale::En => "add mirror" });
msg!(sync_now_button() { Locale::Es => "sincronizar ahora", Locale::En => "sync now" });
msg!(delete_mirror_button() { Locale::Es => "eliminar", Locale::En => "delete" });
msg!(delete_mirror_confirm(label: &str) {
    Locale::Es => "¿eliminar el espejo '{label}'? no se puede deshacer.",
    Locale::En => "delete mirror '{label}'? this cannot be undone.",
});
msg!(add_mirror_button() { Locale::Es => "agregar espejo", Locale::En => "add mirror" });
msg!(save_mirror_button() { Locale::Es => "guardar espejo", Locale::En => "save mirror" });
msg!(mirror_token_shown_once_prefix() {
    Locale::Es => "Token guardado para \"",
    Locale::En => "Token saved for \"",
});
msg!(mirror_token_shown_once_suffix() {
    Locale::Es => "\" — se muestra una sola vez, guardalo ahora:",
    Locale::En => "\" — shown only once, save it now:",
});
msg!(mirror_field_label_sync_merge_requests() {
    Locale::Es => "sincronizar merge requests",
    Locale::En => "sync merge requests",
});
msg!(linked_mrs_heading() { Locale::Es => "merge requests vinculados", Locale::En => "linked merge requests" });
msg!(no_linked_mrs_hint() { Locale::Es => "(sin merge requests vinculados todavia)", Locale::En => "(no linked merge requests yet)" });
msg!(mr_link_table_header_local() { Locale::Es => "local", Locale::En => "local" });
msg!(mr_link_table_header_external() { Locale::Es => "externa", Locale::En => "external" });
msg!(mr_link_table_header_cursor() { Locale::Es => "ultimo sync", Locale::En => "last synced" });
