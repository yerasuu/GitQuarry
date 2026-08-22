//! OIDC login-flow messages — `webui::auth::web::login`'s
//! `handler_login_oidc`/`handler_callback` and the flow-cookie checks around them.

use super::super::{Locale, msg};

msg!(
    /// `auth::web::login::{handler_login_oidc,handler_callback}`: `ctx.oidc` is
    /// `None` but the route was hit anyway (stale link, or OIDC got
    /// disabled after the page rendered). Distinct from `auth::oidc`'s own
    /// identical-text startup error (that one has no `Locale` — discovery
    /// runs once at boot, not from a request).
    oidc_not_configured() {
        Locale::Es => "OIDC no esta configurado en este server",
        Locale::En => "OIDC isn't configured on this server",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP itself reported an error
    /// (denied consent, misconfiguration, etc.) via `?error=`/
    /// `?error_description=`.
    oidc_provider_rejected_login(err: &str, desc: &str) {
        Locale::Es => "el provider OIDC rechazo el login: {err} {desc}",
        Locale::En => "the OIDC provider rejected the login: {err} {desc}",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP redirected back with no
    /// `?code=` param.
    missing_code_param() {
        Locale::Es => "falta ?code=",
        Locale::En => "missing ?code=",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP redirected back with no
    /// `?state=` param.
    missing_state_param() {
        Locale::Es => "falta ?state=",
        Locale::En => "missing ?state=",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: no `oidc-flow` cookie at all
    /// (expired, or the callback was hit without starting at
    /// `/_login_oidc` first).
    no_oidc_login_in_progress() {
        Locale::Es => "no hay un login OIDC en curso (cookie de flujo ausente o vencida)",
        Locale::En => "no OIDC login in progress (missing or expired flow cookie)",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the `oidc-flow` cookie's base64
    /// didn't decode.
    invalid_oidc_flow_cookie() {
        Locale::Es => "cookie de flujo OIDC invalida",
        Locale::En => "invalid OIDC flow cookie",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the decrypted `oidc-flow` cookie
    /// didn't deserialize.
    corrupt_oidc_flow_cookie() {
        Locale::Es => "cookie de flujo OIDC corrupta",
        Locale::En => "corrupt OIDC flow cookie",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the flow cookie's own `csrf` value
    /// doesn't match the IdP's returned `state`.
    oidc_state_mismatch() {
        Locale::Es => "state no coincide (posible CSRF, o el login tardo demasiado)",
        Locale::En => "state mismatch (possible CSRF, or the login took too long)",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: building the HTTP client for the
    /// code/token exchange failed. Same text as `auth::oidc::discover`'s
    /// startup-time error — kept separate since that one has no `Locale`
    /// (runs once at boot, not from a request).
    failed_to_build_oidc_client() {
        Locale::Es => "no pude armar el cliente HTTP para OIDC",
        Locale::En => "couldn't build the HTTP client for OIDC",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: exchanging the authorization code
    /// for a token failed against the IdP.
    code_token_exchange_failed() {
        Locale::Es => "intercambio de code por token fallo",
        Locale::En => "exchanging the code for a token failed",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the token response had no
    /// `id_token` at all.
    oidc_missing_id_token() {
        Locale::Es => "el provider OIDC no devolvio id_token",
        Locale::En => "the OIDC provider didn't return an id_token",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the `id_token`'s signature/claims
    /// didn't verify.
    failed_to_verify_id_token() {
        Locale::Es => "no pude verificar el id_token",
        Locale::En => "couldn't verify the id_token",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the IdP explicitly asserted
    /// `email_verified: false`.
    oidc_email_not_verified() {
        Locale::Es => "el provider OIDC marco este email como no verificado",
        Locale::En => "the OIDC provider marked this email as unverified",
    }
);

msg!(
    /// `auth::web::login::handler_callback`: the verified `id_token` had no
    /// `email` claim (the provider needs the `email` scope requested).
    id_token_missing_email_claim() {
        Locale::Es => "el id_token no tiene claim 'email' (agregar scope 'email' del lado del provider)",
        Locale::En => "the id_token has no 'email' claim (add the 'email' scope on the provider side)",
    }
);
