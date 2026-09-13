//! Auth-domain messages: user/group/session/security/TOTP/preferences —
//! reused across `auth::domain::{user,group}` and `webui::auth::web::*`.

use super::super::{Locale, msg};

msg!(
    /// A username that doesn't resolve to any existing `User` — confirmed
    /// duplicated verbatim across `auth::domain::group`/`auth::domain::user`/
    /// `domain::repo`/`auth::web::favorites`.
    user_not_found(username: &str) {
        Locale::Es => "usuario '{username}' no existe",
        Locale::En => "user '{username}' doesn't exist",
    }
);

msg!(
    /// Placeholder for a uid-keyed reference (`Group::owners`/
    /// `Team::members`) whose `User` no longer resolves — confirmed
    /// duplicated verbatim across `auth::web::groups`'s member list
    /// (`render_group`) and `auth::web::users`'s own rows.
    user_deleted() {
        Locale::Es => "(usuario eliminado)",
        Locale::En => "(deleted user)",
    }
);

msg!(
    /// A pubkey fingerprint already claimed by a different user — confirmed
    /// duplicated verbatim across `auth::domain::user::{create,add_pubkey}`.
    key_already_associated() {
        Locale::Es => "esa clave ya esta asociada a otro usuario",
        Locale::En => "that key is already associated with another user",
    }
);

msg!(
    /// `auth::domain::user::reject_if_claimed_as_secondary`: the address is
    /// already somebody's additional email, so claiming it as a primary
    /// would put the same address in two indexes at once.
    email_claimed_as_secondary() {
        Locale::Es => "esa direccion ya figura como correo adicional de una cuenta; quitala de alli primero",
        Locale::En => "that address is already an account's additional email; remove it there first",
    }
);

msg!(
    /// `auth::domain::user::add_secondary_email`: the email is already
    /// claimed by a different user, either as their primary `email` or one
    /// of their own `secondary_emails`.
    email_already_associated() {
        Locale::Es => "ese email ya esta asociado a otro usuario",
        Locale::En => "that email is already associated with another user",
    }
);

msg!(
    /// A `perm` form field that isn't one of the five
    /// `auth::access::Permission` tier names — confirmed
    /// duplicated verbatim across `auth::domain::group::set_member`,
    /// `auth::domain::user::parse_repos_field`.
    invalid_permission(other: &str) {
        Locale::Es => "permiso invalido '{other}' (usar guest, reporter, developer, maintainer u owner)",
        Locale::En => "invalid permission '{other}' (use guest, reporter, developer, maintainer, or owner)",
    }
);

msg!(
    /// A TOTP code that doesn't verify against the stored secret — confirmed
    /// duplicated verbatim across `auth::domain::user::confirm_totp`,
    /// `auth::web::login`'s post-password TOTP step.
    incorrect_code() {
        Locale::Es => "codigo incorrecto",
        Locale::En => "incorrect code",
    }
);

msg!(
    /// `enable_totp` called while 2FA is already on — single call site,
    /// converted alongside its sibling [`incorrect_code`] check in the
    /// same file rather than left half-converted.
    totp_already_enabled() {
        Locale::Es => "2FA ya esta activo — desactivalo antes de generar un secreto nuevo",
        Locale::En => "2FA is already on — turn it off before generating a new secret",
    }
);

msg!(
    /// `confirm_totp` called with no pending (unconfirmed) secret to check
    /// `code` against — single call site, same file as the two above.
    no_pending_totp_enrollment() {
        Locale::Es => "no hay una inscripcion 2FA pendiente",
        Locale::En => "no pending 2FA enrollment",
    }
);

msg!(
    /// A bulk `"repo:perm"` field entry missing its `:` separator — single
    /// call site (`auth::domain::user::parse_repos_field`), converted alongside
    /// its sibling [`invalid_permission`] check in the same function.
    invalid_entry(entry: &str) {
        Locale::Es => "entrada invalida (esperaba 'repo:read' o 'repo:write'): {entry}",
        Locale::En => "invalid entry (expected 'repo:read' or 'repo:write'): {entry}",
    }
);

msg!(
    /// The session's own `User` record vanished between the request's own
    /// login check and a later re-fetch (rare — a concurrent delete) —
    /// confirmed duplicated verbatim across `auth::web::favorites` (x2),
    /// `auth::web::preferences`, `auth::web::security`.
    account_no_longer_exists() {
        Locale::Es => "tu usuario ya no existe",
        Locale::En => "your account no longer exists",
    }
);

msg!(
    /// A username/group name collision at creation time — confirmed
    /// duplicated verbatim across `auth::web::groups`, `auth::web::users`.
    name_already_taken(name: &str) {
        Locale::Es => "'{name}' ya esta en uso (usuario o grupo)",
        Locale::En => "'{name}' is already taken (by a user or a group)",
    }
);

msg!(
    /// A username rate-limited by `LoginAttempts` — confirmed duplicated
    /// verbatim across `auth::web::login`'s password-form and TOTP-step handlers.
    too_many_failed_attempts() {
        Locale::Es => "demasiados intentos fallidos para este usuario — esperá unos minutos y volvé a intentar",
        Locale::En => "too many failed attempts for this account — wait a few minutes and try again",
    }
);

msg!(
    /// Wrong username or password on the local login form — single call
    /// site, converted alongside its sibling
    /// [`too_many_failed_attempts`] check in the same handler.
    wrong_username_or_password() {
        Locale::Es => "usuario o password incorrectos",
        Locale::En => "wrong username or password",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`'s `add_group` permission gate.
    only_admin_creates_groups() {
        Locale::Es => "solo un admin puede crear grupos",
        Locale::En => "only an admin can create groups",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`'s `delete_group` permission gate.
    only_admin_deletes_groups() {
        Locale::Es => "solo un admin puede borrar grupos",
        Locale::En => "only an admin can delete groups",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`'s `set_storage_quota`
    /// permission gate — same reasoning `auth::web::users`'s whole route
    /// being admin-only already has: an owner shouldn't self-raise their
    /// own allowance.
    only_admin_sets_storage_quota() {
        Locale::Es => "solo un admin puede fijar el limite de almacenamiento",
        Locale::En => "only an admin can set the storage quota",
    }
);

msg!(
    /// `auth::web::groups::apply_groups_action`: `me` isn't an owner of the named
    /// group (nor a global admin) for an action that requires managing it.
    you_dont_manage_group(group: &str) {
        Locale::Es => "no administras el grupo '{group}'",
        Locale::En => "you don't manage the group '{group}'",
    }
);

msg!(
    /// A group name that doesn't resolve to any existing `Group` —
    /// distinct from `user_not_found`/`repo_not_found`, only one call
    /// site (`auth::web::groups`) so no duplicate to collapse here.
    group_not_found(group: &str) {
        Locale::Es => "grupo '{group}' no existe",
        Locale::En => "group '{group}' doesn't exist",
    }
);

msg!(
    /// `auth::web::favorites::handler_favorites_page`: the target user hasn't
    /// opted into `favorites_public`, so their starred list is private.
    favorites_not_public(username: &str) {
        Locale::Es => "los favoritos de '{username}' no son publicos",
        Locale::En => "'{username}''s favorites aren't public",
    }
);

msg!(
    /// `auth::web::security`'s `disable_totp` re-auth gate: neither the account
    /// password nor a live TOTP code matched. Distinct from
    /// `wrong_username_or_password` (login form) — different wording,
    /// different call site, not a verbatim duplicate.
    wrong_password_or_code() {
        Locale::Es => "password o codigo incorrecto",
        Locale::En => "wrong password or code",
    }
);

msg!(
    /// `settings::web::require_admin`: a logged-in non-admin hit an
    /// admin-only page (`/_settings`, `/_users`, a repo's settings tab).
    account_is_not_admin() {
        Locale::Es => "tu usuario no es admin",
        Locale::En => "your account isn't an admin",
    }
);

msg!(
    /// `settings::web::require_admin_or_federation_moderator`: a logged-in
    /// user who is neither a global admin nor `User::is_federation_moderator`
    /// hit a federation-moderation-only page.
    account_is_not_admin_or_moderator() {
        Locale::Es => "tu usuario no es admin ni moderador de federacion",
        Locale::En => "your account is neither an admin nor a federation moderator",
    }
);

msg!(
    /// `auth::domain::user::set_own_email` refusing while OIDC is
    /// configured — the address is a login identity there, so only an admin
    /// assigns it.
    primary_email_is_admin_managed() {
        Locale::Es => "con OIDC configurado, el correo principal solo lo puede cambiar un admin",
        Locale::En => "with OIDC configured, only an admin can change the primary email",
    }
);

msg!(
    /// `auth::web::users::apply_users_action`'s `add_repo_perm`: the `repo` field
    /// was submitted empty.
    missing_repo() {
        Locale::Es => "falta el repo",
        Locale::En => "missing repo",
    }
);

msg!(
    /// `auth::web::preferences::handler_preferences_post`'s `set_text_size_pref`:
    /// `TextSize::parse` rejected the submitted value.
    invalid_size(value: &str) {
        Locale::Es => "tamaño invalido: {value}",
        Locale::En => "invalid size: {value}",
    }
);

msg!(
    /// `auth::web::preferences::handler_preferences_post`'s `set_theme_pref`: not
    /// one of the 3 accepted values (`""`/`"dark"`/`"light"`).
    invalid_theme(value: &str) {
        Locale::Es => "tema invalido: {value}",
        Locale::En => "invalid theme: {value}",
    }
);

msg!(
    /// `auth::web::preferences::handler_preferences_post`'s `set_density_pref`:
    /// not one of the 3 accepted values (`""`/`"dense"`/`"airy"`).
    invalid_density(value: &str) {
        Locale::Es => "densidad invalida: {value}",
        Locale::En => "invalid density: {value}",
    }
);

msg!(
    /// `auth::web::appearance::handler_appearance_post`'s `set_locale_pref`:
    /// not one of the 3 accepted values (`""`/`"es"`/`"en"`, the same set
    /// `gitquarry_i18n::Locale::parse` plus the empty "no pick yet" sentinel
    /// accepts).
    invalid_locale(value: &str) {
        Locale::Es => "idioma invalido: {value}",
        Locale::En => "invalid locale: {value}",
    }
);

msg!(
    /// `auth::web::login`: a session/pending-totp/oidc-flow cookie's `HeaderValue`
    /// failed to parse — confirmed duplicated verbatim across 5 sites
    /// (`handler_login_local` x2, `handler_login_totp_post` x2,
    /// `handler_login_oidc`, `handler_callback` x2 — 7 total call sites).
    building_set_cookie() {
        Locale::Es => "armando Set-Cookie",
        Locale::En => "building Set-Cookie",
    }
);

msg!(
    /// `auth::web::login::handler_login_totp_post`: no `pending-totp` cookie at
    /// all (expired, or navigated to `/_login_totp` directly).
    no_totp_login_in_progress() {
        Locale::Es => "no hay un login en curso (cookie vencida) — volve a /_login",
        Locale::En => "no login in progress (expired cookie) — go back to /_login",
    }
);

msg!(
    /// `auth::web::login::handler_login_totp_post`: the `pending-totp` cookie's
    /// base64 didn't decode.
    invalid_totp_cookie() {
        Locale::Es => "cookie invalida",
        Locale::En => "invalid cookie",
    }
);

msg!(
    /// `auth::web::login::handler_login_totp_post`: the decrypted `pending-totp`
    /// cookie didn't deserialize.
    corrupt_totp_cookie() {
        Locale::Es => "cookie corrupta",
        Locale::En => "corrupt cookie",
    }
);

msg!(
    /// A subgroup that would nest past `access::MAX_GROUP_DEPTH` — single
    /// call site (`auth::domain::group::create`), rejected there because
    /// `ancestor_group_names` would stop short of the new group and silently
    /// drop its members from every permission check on its repos.
    group_nesting_too_deep(max: usize) {
        Locale::Es => "grupos anidados demasiado profundo (maximo {max} niveles)",
        Locale::En => "groups nested too deep (max {max} levels)",
    }
);

msg!(
    /// Deleting a group that still has subgroups — single call site
    /// (`auth::domain::group::delete`); their names are paths under this
    /// one, so freeing the namespace would let someone else claim it and
    /// inherit membership over them.
    group_still_has_subgroups(count: usize) {
        Locale::Es => "el grupo todavia tiene {count} subgrupo(s) — eliminalos primero",
        Locale::En => "this group still has {count} subgroup(s) — delete those first",
    }
);
