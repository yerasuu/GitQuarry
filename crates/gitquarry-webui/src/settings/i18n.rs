//! `settings::web`'s page copy.

use gitquarry_i18n::{Locale, msg};

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
