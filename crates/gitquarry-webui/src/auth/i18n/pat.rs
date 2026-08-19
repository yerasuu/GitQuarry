//! `auth::web::pat`'s page copy.

use gitquarry_i18n::{Locale, msg};

msg!(page_intro() {
    Locale::Es => "Tokens de larga duración para clonar/hacer push por HTTPS con scopes explícitos por repositorio.",
    Locale::En => "Long-lived tokens for HTTPS clone/push with explicit per-repo scopes.",
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

msg!(create_heading() { Locale::Es => "crear token", Locale::En => "create token" });
msg!(label_placeholder() { Locale::Es => "etiqueta (p. ej. \"laptop de trabajo\")", Locale::En => "label (e.g. \"work laptop\")" });
msg!(scopes_placeholder() {
    Locale::Es => "repo:developer, otro/repo:reporter",
    Locale::En => "repo:developer, other/repo:reporter",
});
msg!(expires_days_placeholder() { Locale::Es => "expira en (días, opcional)", Locale::En => "expires in (days, optional)" });
msg!(create_button() { Locale::Es => "crear token", Locale::En => "create token" });
msg!(tokens_heading() { Locale::Es => "tus tokens", Locale::En => "your tokens" });
