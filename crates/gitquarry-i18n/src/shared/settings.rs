//! Server-wide settings messages: external-mirror host allowlist, OIDC config
//! validation, and the first-boot bootstrap-token gate.

use super::super::{Locale, msg};

msg!(
    /// A mirror's `remote_url` points at a host the instance doesn't allow
    /// external mirrors to target. Names the settings page rather than just
    /// refusing: the fix is an admin's, not the person who hit this.
    mirror_host_not_allowed(host: &str) {
        Locale::Es => "el host {host} no esta permitido para mirrors externos — un admin tiene que agregarlo en /_settings.",
        Locale::En => "host {host} isn't allowed for external mirrors — an admin has to add it in /_settings.",
    }
);

msg!(
    /// One entry of the instance's external-mirror host allowlist doesn't
    /// look like a host at all. Shared because both edges reject it: the
    /// settings form that stores the list, and the mirror save that
    /// matches a URL against it.
    invalid_mirror_host_pattern(host: &str) {
        Locale::Es => "host de mirror invalido: {host}. Solo nombres ASCII (letras, digitos, `.`, `-`), con `*.` delante opcional.",
        Locale::En => "invalid mirror host: {host}. ASCII names only (letters, digits, `.`, `-`), optionally prefixed with `*.`.",
    }
);

msg!(
    /// `settings::web::handler_settings_post`'s OIDC "all 4 fields or none"
    /// validation.
    oidc_fields_go_together() {
        Locale::Es => "OIDC: los 4 campos (issuer/client_id/client_secret/redirect_base_url) van juntos o ninguno",
        Locale::En => "OIDC: the 4 fields (issuer/client_id/client_secret/redirect_base_url) go together or none at all",
    }
);

msg!(
    /// `webui::bootstrap::handler_bootstrap_get` on an instance that already
    /// has a login method: the first-boot window is over, and the token
    /// can't reopen it.
    bootstrap_already_done() {
        Locale::Es => "esta instancia ya tiene un metodo de login configurado",
        Locale::En => "this instance already has a login method configured",
    }
);

msg!(
    /// `webui::bootstrap::handler_bootstrap_get` with a wrong or missing
    /// token — deliberately says nothing about which.
    bootstrap_token_invalid() {
        Locale::Es => "token de arranque invalido",
        Locale::En => "invalid bootstrap token",
    }
);

msg!(
    /// `settings::web::require_admin`/`repo_settings::require_repo_authority`
    /// during the first-boot window, for a request carrying no bootstrap
    /// grant: the admin pages need the token the process printed at startup.
    bootstrap_grant_required() {
        Locale::Es => "esta instancia todavia no tiene login configurado: abri /_bootstrap con el token que el proceso imprimio al arrancar",
        Locale::En => "this instance has no login configured yet: open /_bootstrap with the token the process printed at startup",
    }
);
