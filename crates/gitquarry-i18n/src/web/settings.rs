//! `webui::settings::web`'s page copy.

use super::super::{Locale, msg};

msg!(settings_saved_in_bucket_prefix() {
    Locale::Es => "Guardado en ",
    Locale::En => "Saved in ",
});
msg!(settings_saved_in_bucket_suffix() {
    Locale::Es => " dentro del bucket. Todo esto solo aplica al (re)iniciar; un flag de la CLI siempre gana sobre lo guardado aca.",
    Locale::En => " inside the bucket. All of this only applies on (re)start; a CLI flag always wins over what's saved here.",
});

msg!(settings_footer_heading() { Locale::Es => "pie de pagina", Locale::En => "footer" });
msg!(settings_source_link_prose() {
    Locale::Es => "El enlace \"codigo fuente\" del pie de pagina. Un repo ya alojado aca (namespace/nombre) o una URL http(s) completa. Vacio lo oculta.",
    Locale::En => "The footer's \"source\" link. A repo already hosted here (namespace/name) or a full http(s) URL. Empty hides it.",
});
msg!(settings_source_link_label() { Locale::Es => "enlace de codigo fuente", Locale::En => "source link" });

msg!(settings_network_heading() { Locale::Es => "red", Locale::En => "network" });
msg!(settings_web_bind_label() { Locale::Es => "web-bind", Locale::En => "web-bind" });
msg!(settings_ssh_bind_label() { Locale::Es => "ssh-bind", Locale::En => "ssh-bind" });

msg!(settings_mirrors_cache_heading() { Locale::Es => "cache de mirrors", Locale::En => "mirrors cache" });
msg!(settings_mirrors_dir_label() { Locale::Es => "mirrors-dir", Locale::En => "mirrors-dir" });
msg!(settings_mirrors_dir_placeholder() {
    Locale::Es => "(por defecto: ~/.cache/gitquarry/mirrors/<bucket>)",
    Locale::En => "(default: ~/.cache/gitquarry/mirrors/<bucket>)",
});
msg!(settings_mirrors_max_age_label() { Locale::Es => "mirrors-max-age", Locale::En => "mirrors-max-age" });
msg!(settings_mirrors_max_age_placeholder() {
    Locale::Es => "3d (numero + s/m/h/d)",
    Locale::En => "3d (number + s/m/h/d)",
});
msg!(settings_mirrors_max_size_label() { Locale::Es => "mirrors-max-size", Locale::En => "mirrors-max-size" });
msg!(settings_mirrors_max_size_placeholder() {
    Locale::Es => "5G (numero + B/K/M/G)",
    Locale::En => "5G (number + B/K/M/G)",
});

msg!(settings_external_mirrors_heading() {
    Locale::Es => "mirrors externos",
    Locale::En => "external mirrors",
});
msg!(settings_mirror_hosts_prose() {
    Locale::Es => "Hosts adicionales a los que un repo puede espejar, uno por linea o separados por comas. Acepta `*.ejemplo.com`. Para que sirve: configurar un mirror hace que el servidor abra conexiones al host que elija quien lo configure, y esta lista es lo unico que decide a cuales — por eso un forge autohospedado hay que nombrarlo aca.",
    Locale::En => "Extra hosts a repo may mirror to, one per line or comma-separated. `*.example.com` works. What it's for: configuring a mirror makes the server open connections to whatever host the configurer picks, and this list is the only thing deciding which — so a self-hosted forge has to be named here.",
});
msg!(settings_mirror_hosts_builtin_prefix() {
    Locale::Es => "Siempre permitidos, sin configurar nada: ",
    Locale::En => "Always allowed, with nothing configured: ",
});
msg!(settings_mirror_hosts_builtin_suffix() {
    Locale::Es => " — los forges que este build sabe hablar.",
    Locale::En => " — the forges this build knows how to talk to.",
});
msg!(settings_mirror_hosts_label() { Locale::Es => "hosts adicionales", Locale::En => "extra hosts" });

msg!(settings_oidc_heading() { Locale::Es => "login OIDC", Locale::En => "OIDC login" });
// Split around the `code`/`a` elements the paragraph embeds — see
// `webui::settings::render_settings_form` for how these five stitch back
// together with the fixed `/_settings`/`/_users`/`private` markup between
// them.
msg!(settings_oidc_intro_gates() {
    Locale::Es => "Los 4 campos van juntos o ninguno. Habilita ",
    Locale::En => "All 4 fields go together or none. Gates ",
});
msg!(settings_oidc_intro_tab() {
    Locale::Es => "cada pestana \"ajustes\" de repo, y deja que alguien que entra con un email que matchea un usuario en ",
    Locale::En => "every repo's \"settings\" tab, and lets someone who logs in with an email matching a user in ",
});
msg!(settings_oidc_intro_see() {
    Locale::Es => " vea repos ",
    Locale::En => " see ",
});
msg!(settings_oidc_intro_private() {
    Locale::Es => " en la web. Alternativa/complemento sin un IdP externo: password por usuario, habilitado desde ",
    Locale::En => " repos on the web. Alternative/complement without an external IdP: per-user password, enabled from ",
});
msg!(settings_oidc_intro_see_there() {
    Locale::Es => " (ver ahi).",
    Locale::En => " (see there).",
});

msg!(settings_issuer_url_label() { Locale::Es => "issuer url", Locale::En => "issuer url" });
msg!(settings_client_id_label() { Locale::Es => "client id", Locale::En => "client id" });
msg!(settings_client_secret_label() { Locale::Es => "client secret", Locale::En => "client secret" });
// Placeholders for the write-only client-secret field -- see
// `crate::settings::web::handler_settings_post` on why it is never
// rendered back with a value.
msg!(settings_client_secret_placeholder_unchanged() {
    Locale::Es => "guardado - dejar en blanco para no cambiarlo",
    Locale::En => "stored - leave blank to keep it",
});
msg!(settings_client_secret_placeholder_unset() {
    Locale::Es => "sin configurar",
    Locale::En => "not set",
});
msg!(settings_redirect_base_url_label() { Locale::Es => "redirect base url", Locale::En => "redirect base url" });
msg!(settings_redirect_base_url_placeholder() {
    Locale::Es => "https://git.example.com (publica, sin barra final)",
    Locale::En => "https://git.example.com (public, no trailing slash)",
});

msg!(settings_save_button() { Locale::Es => "guardar", Locale::En => "save" });
msg!(settings_oidc_save_confirm() {
    Locale::Es => "¿guardar la configuracion OIDC? un valor erroneo puede romper el login para todos hasta corregirlo directamente en el bucket.",
    Locale::En => "save OIDC config? a bad value can break login for everyone until fixed directly in the bucket.",
});

msg!(settings_backup_heading() { Locale::Es => "backup", Locale::En => "backup" });
msg!(settings_backup_prose() {
    Locale::Es => "Copia toda la instancia a otro bucket, a un directorio, o a este mismo bucket. El destino se nombra, nunca se escriben credenciales aca: viven en gitquarry.toml.",
    Locale::En => "Copies the whole instance to another bucket, a directory, or this same bucket. The destination is named, never credentialed here: credentials live in gitquarry.toml.",
});
msg!(settings_backup_dest_label() { Locale::Es => "destino", Locale::En => "destination" });
msg!(settings_backup_dest_placeholder() {
    Locale::Es => "bucket:offsite | path:/srv/backup | self",
    Locale::En => "bucket:offsite | path:/srv/backup | self",
});
msg!(settings_backup_dest_hint() {
    Locale::Es => "bucket:<nombre> es una seccion [buckets.<nombre>] de gitquarry.toml y es el unico destino que sobrevive perder la cuenta del proveedor. self solo hace snapshot, no replica.",
    Locale::En => "bucket:<name> is a [buckets.<name>] section of gitquarry.toml, and the only destination that survives losing the provider account. self only snapshots, it never replicates.",
});
msg!(settings_backup_enabled_hint() {
    Locale::Es => "ejecutar el backup periodico",
    Locale::En => "run the backup on a schedule",
});
msg!(settings_backup_interval_label() { Locale::Es => "intervalo", Locale::En => "interval" });
msg!(settings_backup_interval_placeholder() {
    Locale::Es => "24h (numero + s/m/h/d, minimo 1h)",
    Locale::En => "24h (number + s/m/h/d, minimum 1h)",
});
msg!(settings_backup_snapshot_hint() {
    Locale::Es => "guardar tambien un snapshot versionado de la config (lo unico que conserva historial)",
    Locale::En => "also keep a versioned snapshot of the config (the only half that keeps history)",
});
msg!(settings_backup_classes_heading() { Locale::Es => "que se copia", Locale::En => "what gets copied" });
msg!(settings_backup_class_git_hint() { Locale::Es => "git (packs y refs)", Locale::En => "git (packs and refs)" });
msg!(settings_backup_class_lfs_hint() { Locale::Es => "objetos LFS", Locale::En => "LFS objects" });
msg!(settings_backup_class_releases_hint() { Locale::Es => "releases y sus assets", Locale::En => "releases and their assets" });
msg!(settings_backup_class_avatars_hint() { Locale::Es => "avatares", Locale::En => "avatars" });
msg!(settings_backup_class_entities_hint() {
    Locale::Es => "config, usuarios, grupos, PATs, permisos (necesario para levantar la instancia)",
    Locale::En => "config, users, groups, PATs, permissions (needed to bring the instance back up)",
});
msg!(settings_backup_class_audit_hint() { Locale::Es => "registro de auditoria", Locale::En => "audit trail" });
msg!(settings_backup_class_derived_hint() {
    Locale::Es => "caches regenerables (indice de busqueda, firmas). Apagado se reconstruyen con backfill-repo-index tras un restore.",
    Locale::En => "rebuildable caches (search index, signatures). Off, they get rebuilt with backfill-repo-index after a restore.",
});
msg!(settings_backup_retention_heading() { Locale::Es => "retencion", Locale::En => "retention" });
msg!(settings_backup_keep_label() { Locale::Es => "snapshots a conservar", Locale::En => "snapshots to keep" });
msg!(settings_backup_keep_placeholder() {
    Locale::Es => "vacio = conservar todos",
    Locale::En => "empty = keep every one",
});
msg!(settings_backup_audit_keep_label() { Locale::Es => "meses de auditoria", Locale::En => "audit months" });
msg!(settings_backup_audit_keep_placeholder() {
    Locale::Es => "vacio = conservar todos",
    Locale::En => "empty = keep every one",
});
msg!(settings_backup_delete_grace_label() { Locale::Es => "dias de gracia al borrar", Locale::En => "delete grace days" });
msg!(settings_backup_delete_grace_placeholder() {
    Locale::Es => "vacio = nunca borrar del destino",
    Locale::En => "empty = never delete at the destination",
});
msg!(settings_backup_delete_grace_hint() {
    Locale::Es => "Un objeto borrado en origen sobrevive estos dias en el destino. Vacio (por defecto) es lo que hace recuperable un borrado accidental.",
    Locale::En => "An object deleted at the source survives this many days at the destination. Empty (the default) is what makes an accidental deletion recoverable.",
});
msg!(settings_backup_concurrency_label() { Locale::Es => "concurrencia", Locale::En => "concurrency" });
msg!(settings_backup_concurrency_placeholder() { Locale::Es => "8", Locale::En => "8" });
msg!(settings_backup_run_now_button() { Locale::Es => "ejecutar ahora", Locale::En => "run now" });
msg!(settings_backup_run_started() {
    Locale::Es => "backup lanzado en segundo plano; recarga para ver el resultado.",
    Locale::En => "backup started in the background; reload to see how it went.",
});
msg!(settings_backup_runs_heading() { Locale::Es => "ejecuciones recientes", Locale::En => "recent runs" });
msg!(settings_backup_runs_empty() {
    Locale::Es => "todavia no se ha ejecutado ningun backup.",
    Locale::En => "no backup has run yet.",
});
msg!(settings_backup_col_started() { Locale::Es => "inicio", Locale::En => "started" });
msg!(settings_backup_col_outcome() { Locale::Es => "resultado", Locale::En => "outcome" });
msg!(settings_backup_col_duration() { Locale::Es => "duracion", Locale::En => "duration" });
msg!(settings_backup_col_copied() { Locale::Es => "copiados", Locale::En => "copied" });
msg!(settings_backup_col_skipped() { Locale::Es => "omitidos", Locale::En => "skipped" });
msg!(settings_backup_col_failed() { Locale::Es => "fallidos", Locale::En => "failed" });
msg!(settings_backup_col_trigger() { Locale::Es => "origen", Locale::En => "trigger" });
msg!(settings_backup_unknown_warning() {
    Locale::Es => "objetos sin clasificar copiados (se copian siempre, pero conviene clasificarlos en backup::inventory):",
    Locale::En => "unclassified objects copied (they always are, but they're worth classifying in backup::inventory):",
});
msg!(settings_backup_failures_heading() { Locale::Es => "fallos", Locale::En => "failures" });
msg!(settings_backup_secrets_warning() {
    Locale::Es => "El destino guarda los mismos secretos que este bucket: clave de host ssh, clave de firma, hashes de contrasenas, secretos TOTP, tokens de mirrors. Tratalo como igual de sensible.",
    Locale::En => "The destination holds the same secrets as this bucket: ssh host key, signing key, password hashes, TOTP secrets, mirror tokens. Treat it as just as sensitive.",
});
