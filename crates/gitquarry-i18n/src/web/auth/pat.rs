use super::super::super::{Locale, msg};

msg!(page_intro() {
    Locale::Es => "Tokens de larga duración con scopes explícitos por repositorio: los de git clonan y hacen push por HTTPS, los de API valen solo en /_api/v1. Ninguno vale en el otro, y son distintos de los tokens de federación.",
    Locale::En => "Long-lived tokens with explicit per-repo scopes: git tokens clone and push over HTTPS, API tokens work only on /_api/v1. Neither works on the other, and both are separate from federation's own tokens.",
});
msg!(secret_reveal_warning() {
    Locale::Es => "Copia este token ahora: no se puede volver a ver.",
    Locale::En => "Copy this token now: it can't be shown again.",
});

msg!(label_column_header() { Locale::Es => "etiqueta", Locale::En => "label" });
msg!(scopes_column_header() { Locale::Es => "scopes", Locale::En => "scopes" });
msg!(expires_column_header() { Locale::Es => "expira", Locale::En => "expires" });
msg!(last_used_column_header() { Locale::Es => "último uso", Locale::En => "last used" });
msg!(no_tokens_yet_hint() { Locale::Es => "(sin tokens todavía)", Locale::En => "(no tokens yet)" });
msg!(revoke_button() { Locale::Es => "revocar", Locale::En => "revoke" });
msg!(revoke_confirm(label: &str) {
    Locale::Es => "¿revocar el token '{label}'? cualquier cosa que lo use dejara de funcionar de inmediato.",
    Locale::En => "revoke token '{label}'? anything using it stops working immediately.",
});

msg!(create_heading() { Locale::Es => "crear token", Locale::En => "create token" });
msg!(label_placeholder() { Locale::Es => "etiqueta (p. ej. \"laptop de trabajo\")", Locale::En => "label (e.g. \"work laptop\")" });
msg!(scopes_placeholder() {
    Locale::Es => "repo:developer, otro/repo:reporter",
    Locale::En => "repo:developer, other/repo:reporter",
});
msg!(expires_days_placeholder() { Locale::Es => "expira en (días, opcional)", Locale::En => "expires in (days, optional)" });
msg!(create_button() { Locale::Es => "crear token", Locale::En => "create token" });
msg!(tokens_heading() { Locale::Es => "tus tokens", Locale::En => "your tokens" });

msg!(kind_column_header() { Locale::Es => "tipo", Locale::En => "kind" });
msg!(kind_git() { Locale::Es => "git (HTTPS)", Locale::En => "git (HTTPS)" });
msg!(kind_api() { Locale::Es => "API", Locale::En => "API" });
msg!(api_scopes_label() { Locale::Es => "scopes de la API", Locale::En => "API scopes" });
msg!(api_scopes_hint() {
    Locale::Es => "Solo para tokens de API. Nunca dan más de lo que tú puedes hacer ni más que el permiso puesto a cada repositorio.",
    Locale::En => "API tokens only. They never give more than you can do, nor more than the permission set on each repo.",
});
msg!(paths_label() { Locale::Es => "rutas (opcional)", Locale::En => "paths (optional)" });
msg!(paths_placeholder() {
    Locale::Es => "repo:servicios/api (una por línea)",
    Locale::En => "repo:services/api (one per line)",
});
msg!(paths_hint() {
    Locale::Es => "Solo para tokens de API: limita un repositorio a esos subdirectorios. Un repositorio sin rutas queda entero.",
    Locale::En => "API tokens only: confines a repo to those subdirectories. A repo with no paths stays open in full.",
});
