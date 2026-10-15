//! Messages reused across 2+ call sites — collapsing a currently-duplicated
//! Spanish literal onto one function here, instead of translating it
//! separately at each site, is the whole point of this module. Split by domain
//! into submodules ([`auth`], [`oidc`], [`repo`], [`release`], [`merge_request`],
//! [`settings`]); what's left here is genuinely cross-cutting — request/session
//! plumbing and generic UI widgets no single domain owns.

use super::{Locale, msg};

pub mod auth;
pub mod merge_request;
pub mod oidc;
pub mod release;
pub mod repo;
pub mod settings;

msg!(
    /// A `Cas::Conflict` on a bucket-side write: someone else's edit landed
    /// first. Confirmed duplicated verbatim across 9 files before this
    /// (`domain::repo::rename` plus 8 web `Flash::Err` sites) — this is the
    /// first one converted; the rest follow in later commits.
    cas_conflict() {
        Locale::Es => "conflicto guardando (otra edicion concurrente) — recarga y reintenta.",
        Locale::En => "conflict saving (another edit happened concurrently) — reload and retry.",
    }
);

msg!(
    /// A settings form's success flash where nothing more specific applies.
    saved() {
        Locale::Es => "guardado.",
        Locale::En => "saved.",
    }
);

msg!(
    /// A success flash for a write that lands in the bucket itself (users,
    /// groups), as opposed to a per-browser preference.
    saved_to_bucket() {
        Locale::Es => "guardado en el bucket.",
        Locale::En => "saved to the bucket.",
    }
);

msg!(
    /// A confirm-dialog's dismiss action — the footer "Cancel" button and
    /// the header `×` both abandon the dialog the same way, so they share
    /// this one string rather than minting two near-duplicates.
    cancel_button() {
        Locale::Es => "cancelar",
        Locale::En => "cancel",
    }
);

msg!(
    /// A flash message's `×` — acknowledging/removing a one-line status
    /// notice, not the same action `cancel_button` names (abandoning a
    /// dialog before it does anything) even though both render as `×`.
    dismiss_button() {
        Locale::Es => "cerrar",
        Locale::En => "dismiss",
    }
);

msg!(
    /// `webui::page_chrome::copy_button`'s resting label.
    copy_button() {
        Locale::Es => "copiar",
        Locale::En => "copy",
    }
);

msg!(
    /// `webui::page_chrome::copy_button`'s label for the brief window right
    /// after a successful `navigator.clipboard.writeText` (see `app.js`'s
    /// `[data-copy]` handler) — swapped back to `copy_button` after ~1.2s.
    copied_label() {
        Locale::Es => "copiado",
        Locale::En => "copied",
    }
);

msg!(
    /// The `other` arm of a form action `match` — a submitted `action`
    /// field that doesn't name any action the handler knows. Confirmed
    /// duplicated verbatim across `auth::web::preferences`/`auth::web::groups`/
    /// `auth::web::favorites`/`auth::web::security`/`auth::web::users`.
    unknown_action(other: &str) {
        Locale::Es => "accion desconocida: {other}",
        Locale::En => "unknown action: {other}",
    }
);

msg!(
    /// `webui::require_csrf`: the submitted `csrf` field didn't match this
    /// session's cookie (missing, expired, or a stale form). No `ctx` in
    /// scope here, so this only gets the cookie-override/built-in-default
    /// tiers of locale resolution, not the account-preference middle one.
    invalid_csrf() {
        Locale::Es => "token CSRF invalido o vencido — recarga la pagina y reintenta",
        Locale::En => "invalid or expired CSRF token — reload the page and try again",
    }
);

msg!(
    /// `webui::require_own_profile`: the `{username}` URL segment of a
    /// self-service page doesn't match the logged-in session's own account.
    only_your_own_account() {
        Locale::Es => "solo podes acceder a tu propia cuenta",
        Locale::En => "you can only access your own account",
    }
);

msg!(
    /// What a 5xx tells the client, in place of the underlying error. The
    /// blanket `From<E> for AppError` turns any failure into a 500, so the
    /// real chain is whatever bubbled up — git's stderr, an S3 key, a
    /// mirror path on disk — none of which a client should be handed. The
    /// full chain still goes to the server log; see
    /// `gitquarry_webui::webui::error::AppError::client_message`.
    internal_error() {
        Locale::Es => "error interno del servidor",
        Locale::En => "internal server error",
    }
);
