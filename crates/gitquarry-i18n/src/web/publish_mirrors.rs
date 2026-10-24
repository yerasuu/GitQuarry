//! A repo's "publish mirrors" settings section and its dry-run preview
//! (`docs/publish-mirror-concept.md`, phase 0: nothing is pushed).

use super::super::{Locale, msg};

msg!(heading() { Locale::Es => "espejos de publicación", Locale::En => "publish mirrors" });
msg!(prose() {
    Locale::Es => "Publica una versión filtrada de este repo privado en un remoto público. Fase 0: solo configuración y vista previa; desde aquí no se empuja nada a ningún sitio.",
    Locale::En => "Publishes a filtered version of this private repo to a public remote. Phase 0: configuration and preview only; nothing is pushed anywhere from here.",
});
msg!(none_hint() { Locale::Es => "(sin espejos de publicación)", Locale::En => "(no publish mirrors yet)" });
msg!(add_heading() { Locale::Es => "nuevo espejo de publicación", Locale::En => "new publish mirror" });
msg!(add_button() { Locale::Es => "crear", Locale::En => "create" });
msg!(save_button() { Locale::Es => "guardar", Locale::En => "save" });
msg!(preview_button() { Locale::Es => "vista previa", Locale::En => "preview" });
msg!(delete_button() { Locale::Es => "eliminar", Locale::En => "delete" });
msg!(delete_confirm(label: &str) {
    Locale::Es => "¿eliminar el espejo de publicación '{label}'?",
    Locale::En => "delete publish mirror '{label}'?",
});
msg!(added() { Locale::Es => "espejo de publicación creado.", Locale::En => "publish mirror created." });
msg!(updated() { Locale::Es => "espejo de publicación guardado.", Locale::En => "publish mirror saved." });
msg!(removed() { Locale::Es => "espejo de publicación eliminado.", Locale::En => "publish mirror removed." });
msg!(previewed() { Locale::Es => "vista previa calculada; no se ha empujado nada.", Locale::En => "preview computed; nothing was pushed." });
msg!(bad_identity_line(line: &str) {
    Locale::Es => "línea de identidad no válida (se espera «privado público [nombre]»): {line}",
    Locale::En => "invalid identity line (expected \"private public [name]\"): {line}",
});

msg!(field_label() { Locale::Es => "etiqueta", Locale::En => "label" });
msg!(field_remote_url() { Locale::Es => "URL pública", Locale::En => "public URL" });
msg!(field_mode() { Locale::Es => "modo", Locale::En => "mode" });
msg!(mode_allowlist() { Locale::Es => "lista de inclusión (recomendado)", Locale::En => "allowlist (recommended)" });
msg!(mode_exclude() { Locale::Es => "exclusión", Locale::En => "exclude" });
msg!(field_include() {
    Locale::Es => "incluir (una ruta glob por línea, solo en modo lista de inclusión)",
    Locale::En => "include (one path glob per line, allowlist mode only)",
});
msg!(field_exclude() {
    Locale::Es => "excluir (una ruta glob por línea; siempre gana)",
    Locale::En => "exclude (one path glob per line; always wins)",
});
msg!(field_exclude_submodules() { Locale::Es => "excluir submódulos", Locale::En => "exclude submodules" });
msg!(field_strip_empty_dirs() { Locale::Es => "quitar directorios vacíos", Locale::En => "strip empty directories" });
msg!(field_branches() { Locale::Es => "ramas publicadas (separadas por comas)", Locale::En => "published branches (comma-separated)" });
msg!(field_tags() { Locale::Es => "tags publicados (separados por comas)", Locale::En => "published tags (comma-separated)" });
msg!(field_identities() {
    Locale::Es => "mapa de identidades («privado público [nombre]» por línea)",
    Locale::En => "identity map (\"private public [name]\" per line)",
});
msg!(field_unmapped() { Locale::Es => "identidades sin mapear", Locale::En => "unmapped identities" });
msg!(unmapped_block() { Locale::Es => "bloquear", Locale::En => "block" });
msg!(unmapped_passthrough() { Locale::Es => "dejar pasar", Locale::En => "pass through" });
msg!(field_spdx() { Locale::Es => "identificador SPDX (opcional)", Locale::En => "SPDX id (optional)" });
msg!(field_secret_allowlist() {
    Locale::Es => "falsos positivos del escaneo de secretos («sha [ruta]» por línea; sin ruta = mensaje de commit)",
    Locale::En => "secret-scan false positives (\"sha [path]\" per line; no path = commit message)",
});

msg!(table_label() { Locale::Es => "etiqueta", Locale::En => "label" });
msg!(table_remote() { Locale::Es => "remoto", Locale::En => "remote" });
msg!(table_refs() { Locale::Es => "refs", Locale::En => "refs" });
msg!(table_actions() { Locale::Es => "acciones", Locale::En => "actions" });

msg!(preview_heading(label: &str) { Locale::Es => "vista previa de «{label}»", Locale::En => "preview of \"{label}\"" });
msg!(preview_verdict_pass() {
    Locale::Es => "todas las puertas pasan (nada se ha empujado)",
    Locale::En => "every gate passes (nothing was pushed)",
});
msg!(preview_verdict_blocked() {
    Locale::Es => "bloqueado: una publicación se negaría a empezar",
    Locale::En => "blocked: a publish run would refuse to start",
});
msg!(preview_no_refs() {
    Locale::Es => "ninguna rama ni tag coincide con las refs declaradas.",
    Locale::En => "no branch or tag matches the declared refs.",
});
msg!(private_head() { Locale::Es => "head privado", Locale::En => "private head" });
msg!(public_head() { Locale::Es => "head público resultante", Locale::En => "resulting public head" });
msg!(public_head_none() { Locale::Es => "(nada que publicar)", Locale::En => "(nothing to publish)" });
msg!(published_commits() { Locale::Es => "commits publicados", Locale::En => "published commits" });
msg!(collapsed_commits() { Locale::Es => "commits que colapsan", Locale::En => "commits that collapse" });
msg!(excluded_heading() { Locale::Es => "excluidas, por regla", Locale::En => "excluded, by rule" });
msg!(excluded_none() { Locale::Es => "(nada excluido)", Locale::En => "(nothing excluded)" });
msg!(published_paths(count: usize) { Locale::Es => "rutas publicadas ({count})", Locale::En => "published paths ({count})" });
msg!(ref_error() { Locale::Es => "no se pudo previsualizar", Locale::En => "couldn't preview" });

msg!(rule_config(pattern: &str) { Locale::Es => "configuración: {pattern}", Locale::En => "config: {pattern}" });
msg!(rule_repo_file(pattern: &str) { Locale::Es => "publish.toml: {pattern}", Locale::En => "publish.toml: {pattern}" });
msg!(rule_filter_file() { Locale::Es => "el propio .gitquarry/publish.toml", Locale::En => ".gitquarry/publish.toml itself" });
msg!(rule_submodule() { Locale::Es => "submódulo", Locale::En => "submodule" });
msg!(rule_not_in_allowlist() { Locale::Es => "fuera de la lista de inclusión", Locale::En => "not in the allowlist" });

msg!(gate_filter() { Locale::Es => "auditoría del filtro", Locale::En => "filter audit" });
msg!(gate_blob_set() { Locale::Es => "auditoría de blobs", Locale::En => "blob-set audit" });
msg!(gate_license() { Locale::Es => "licencia", Locale::En => "license" });
msg!(gate_secrets() { Locale::Es => "escaneo de secretos", Locale::En => "secret scan" });
msg!(gate_identity() { Locale::Es => "identidades", Locale::En => "identities" });
msg!(gate_host() { Locale::Es => "host", Locale::En => "host" });
msg!(gate_pass() { Locale::Es => "pasa", Locale::En => "pass" });
msg!(gate_blocked() { Locale::Es => "bloquea", Locale::En => "blocks" });

msg!(finding_repo_filter_invalid(error: &str) {
    Locale::Es => ".gitquarry/publish.toml no se puede usar: {error}",
    Locale::En => ".gitquarry/publish.toml can't be used: {error}",
});
msg!(finding_excluded_path_published(path: &str, rule: &str) {
    Locale::Es => "{path} se publicaría aunque lo excluye {rule}",
    Locale::En => "{path} would be published although {rule} excludes it",
});
msg!(finding_blob_also_excluded(sha: &str, published: &str, excluded: &str) {
    Locale::Es => "el contenido {sha} se publica en {published} y también está en la ruta excluida {excluded}",
    Locale::En => "content {sha} is published at {published} and also sits at the excluded path {excluded}",
});
msg!(finding_license_missing() {
    Locale::Es => "no hay LICENSE ni COPYING en la raíz del árbol publicado",
    Locale::En => "no LICENSE or COPYING at the root of the published tree",
});
msg!(finding_spdx_not_declared(expected: &str) {
    Locale::Es => "el árbol publicado no declara SPDX {expected} (línea SPDX-License-Identifier o LICENSES/{expected}.txt)",
    Locale::En => "the published tree doesn't declare SPDX {expected} (an SPDX-License-Identifier line or LICENSES/{expected}.txt)",
});
msg!(finding_spdx_mismatch(expected: &str, found: &str) {
    Locale::Es => "se esperaba SPDX {expected}, la licencia declara {found}",
    Locale::En => "expected SPDX {expected}, the license declares {found}",
});
msg!(finding_secret(path: &str, sha: &str, rule: &str, line: usize) {
    Locale::Es => "{path} ({sha}), línea {line}: {rule}",
    Locale::En => "{path} ({sha}), line {line}: {rule}",
});
msg!(finding_secret_in_message(sha: &str, rule: &str, line: usize) {
    Locale::Es => "mensaje del commit {sha}, línea {line}: {rule}",
    Locale::En => "message of commit {sha}, line {line}: {rule}",
});
msg!(finding_secret_scan_skipped(path: &str, sha: &str) {
    Locale::Es => "{path} ({sha}) es demasiado grande para escanearlo",
    Locale::En => "{path} ({sha}) is too large to scan",
});
msg!(finding_unmapped_author(email: &str, commits: usize, example: &str) {
    Locale::Es => "autor sin mapear {email} en {commits} commit(s), p. ej. {example}",
    Locale::En => "unmapped author {email} on {commits} commit(s), e.g. {example}",
});
msg!(finding_unmapped_committer(email: &str, commits: usize, example: &str) {
    Locale::Es => "committer sin mapear {email} en {commits} commit(s), p. ej. {example}",
    Locale::En => "unmapped committer {email} on {commits} commit(s), e.g. {example}",
});
msg!(finding_host_not_allowed(error: &str) { Locale::Es => "{error}", Locale::En => "{error}" });
