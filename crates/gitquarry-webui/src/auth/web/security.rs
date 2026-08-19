//! `/_account/security`: self-service credentials for whoever is
//! logged in — SSH pubkeys and password, same idea GitLab/Forgejo call
//! "your SSH keys"/"change password" in a normal user's own settings, as
//! opposed to [`super::users`] (admin-only, edits *any* user's pubkeys/
//! password/`is_admin`/per-repo permissions). Display defaults (text-size,
//! theme) live at [`super::appearance`] instead — split out for the same
//! reason [`super::favorites`] is its own page: a display preference isn't
//! a credential, and mixing the two made this page read like a catch-all
//! instead of what it actually is — the one place that changes how someone
//! can authenticate as this user.
//!
//! No username in the URL — always the logged-in session's own account,
//! resolved server-side via [`login::current_user`], same as every other
//! `/_account/*` section (see `crate::webui::account_settings_page`'s own
//! doc comment). `email` and `is_admin` aren't editable here either: `email`
//! is how OIDC login resolves to a [`User`], letting someone self-assign it
//! could collide with another account's identity; `is_admin` stays an
//! admin-only grant, same as always.

use super::login;
use crate::webui::{
    AppError, Flash, State_, account_settings_page, csrf_field, effective_density, effective_locale, effective_theme,
    federation_ui_active, layout, non_empty, render_flash, require_csrf, require_login,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use gitquarry_auth_crypto::{password, totp};
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::User;
use gitquarry_core::storage::Cas;
use maud::{Markup, PreEscaped, html};

pub(crate) async fn handler_security_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let user = match require_login(&ctx, &headers, "/_account/security").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_security_page(&user, &csrf_field(&headers), None, None, None, locale);
    let body = account_settings_page("security", content, federation_ui_active(&ctx), locale).into_string();
    Ok(axum::response::Html(layout(
        "security",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Shared re-auth gate for `disable_totp`/`regenerate_recovery_codes`:
/// either the account password or a live TOTP code, whichever the user has
/// handy — checked here, not inside `domain::user`, so a stolen session
/// cookie alone can't turn 2FA off or invalidate someone's unused recovery
/// codes (the whole point of having either).
fn require_reauth(me: &User, attempt: &str, locale: gitquarry_i18n::Locale) -> Result<(), String> {
    let password_ok = !attempt.is_empty()
        && me
            .password_hash
            .as_deref()
            .is_some_and(|h| password::verify_password(attempt, h));
    let code_ok = me.totp_secret.as_deref().is_some_and(|s| totp::verify_code(s, attempt));
    if !password_ok && !code_ok {
        return Err(gitquarry_i18n::shared::wrong_password_or_code(locale));
    }
    Ok(())
}

/// [`require_reauth`] plus the same per-username lockout `/_login` applies
/// ([`gitquarry_core::app::LoginAttempts`]). Without it this page is an
/// unmetered guessing oracle for a 6-digit TOTP code: whoever holds a
/// session cookie could grind one out here and use it to turn 2FA off,
/// while the login form's own limiter never sees a single attempt. The
/// same bucket, deliberately, not a second one — guesses made here and at
/// `/_login` are guesses at the same account and should add up.
async fn require_reauth_throttled(
    ctx: &gitquarry_core::app::AppContext,
    me: &User,
    attempt: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(), String> {
    if ctx.login_attempts.is_locked(&me.username) {
        return Err(gitquarry_i18n::shared::too_many_failed_attempts(locale));
    }
    match require_reauth(me, attempt, locale) {
        Ok(()) => {
            ctx.login_attempts.clear(&me.username);
            Ok(())
        }
        Err(e) => {
            ctx.login_attempts.record_failure(&me.username);
            Err(e)
        }
    }
}

/// Same gate as [`require_reauth_throttled`], in front of the two actions that
/// establish a credential outliving the session that created it: a new
/// password (which then logs in on its own) and a new SSH key (which then
/// pushes on its own). Without this a stolen cookie converts itself into
/// permanent access before its 7-day TTL runs out — the same reasoning
/// `disable_totp` and `regenerate_recovery_codes` already apply, which
/// these two were missing.
///
/// Skipped only when there is genuinely nothing to re-prove: an account
/// with neither a password nor a TOTP secret (an OIDC-only user setting a
/// first password, or adding a first key) has no second factor to ask for,
/// and demanding one would lock them out of ever acquiring one.
async fn require_reauth_for_credential_change(
    ctx: &gitquarry_core::app::AppContext,
    me: &User,
    attempt: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(), String> {
    if me.password_hash.is_none() && me.totp_secret.is_none() {
        return Ok(());
    }
    require_reauth_throttled(ctx, me, attempt, locale).await
}

#[derive(serde::Deserialize)]
pub(crate) struct SecurityForm {
    /// Wire name `security_action`, not `action`: `remove_pubkey`/
    /// `remove_gpg_key` are `data-async` -- `HTMLFormElement` is
    /// `[OverrideBuiltins]`, so a control literally named `action` shadows
    /// the form's own `.action` IDL property, and `style/app.js`'s
    /// `fetchFragment(form.action, ...)` got the `<input>` element instead
    /// of the URL, 404-ing. Every form on this page shares this one
    /// struct/field, so the rename applies even to the plain ones too.
    #[serde(rename = "security_action")]
    action: String,
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    pubkey: String,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    secondary_email: String,
    #[serde(default)]
    gpg_key: String,
    #[serde(default)]
    gpg_fingerprint: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    totp_code: String,
    /// Current password or live TOTP code, proving the session holder is
    /// the account holder before a credential change — see
    /// [`require_reauth_for_credential_change`]. Separate from `password`
    /// (the *new* one being set) and from `totp_code` (which the 2FA forms
    /// use for their own re-auth).
    #[serde(default)]
    reauth: String,
}

pub(crate) async fn handler_security_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<SecurityForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/security").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let csrf = csrf_field(&headers);

    // Every action here is scoped to `me.username` (never a form-submitted
    // one, see this module's own doc comment) — same domain::user functions
    // `/_users`' admin actions call, just always targeting the caller's own
    // account.
    //
    // A rejected action (wrong re-auth password/code, invalid pubkey, weak
    // password, ...) re-renders this same page with `Flash::Err` right here,
    // the same way a successful one re-renders it with `Flash::Ok` below —
    // losing the whole page (SSH keys, TOTP state, everything) to a separate
    // generic error page over one bad field would be a much worse outcome
    // than just showing the error where the user can retry.
    macro_rules! fail {
        ($msg:expr) => {
            return Ok(axum::response::Html(layout(
                "security",
                "",
                &account_settings_page(
                    "security",
                    render_security_page(&me, &csrf, Some(Flash::Err(&$msg)), None, None, locale),
                    federation_ui_active(&ctx),
                    locale,
                )
                .into_string(),
                &topnav,
                effective_theme(&ctx, &headers).await,
                effective_density(&ctx, &headers).await,
                locale,
            ))
            .into_response())
        };
    }

    let mut pending_totp_secret = None;
    let mut revealed_recovery_codes = None;
    let cas = match form.action.as_str() {
        "add_pubkey" => {
            if let Err(e) = require_reauth_for_credential_change(&ctx, &me, form.reauth.trim(), locale).await {
                fail!(e);
            }
            match domain::user::add_pubkey(&ctx, &me.username, form.pubkey.trim(), locale).await {
                Ok(cas) => cas,
                Err(e) => fail!(e.to_string()),
            }
        }
        "remove_pubkey" => match domain::user::remove_pubkey(&ctx, &me.username, &form.fingerprint, locale).await {
            Ok(cas) => cas,
            Err(e) => fail!(e.to_string()),
        },
        "add_secondary_email" => {
            match domain::user::add_secondary_email(&ctx, &me.username, form.secondary_email.trim(), locale).await {
                Ok(cas) => cas,
                Err(e) => fail!(e.to_string()),
            }
        }
        "remove_secondary_email" => {
            match domain::user::remove_secondary_email(&ctx, &me.username, form.secondary_email.trim(), locale).await {
                Ok(cas) => cas,
                Err(e) => fail!(e.to_string()),
            }
        }
        "add_gpg_key" => match domain::user::add_gpg_key(&ctx, &me.username, form.gpg_key.trim(), locale).await {
            Ok(cas) => cas,
            Err(e) => fail!(e.to_string()),
        },
        "remove_gpg_key" => match domain::user::remove_gpg_key(&ctx, &me.username, &form.gpg_fingerprint, locale).await
        {
            Ok(cas) => cas,
            Err(e) => fail!(e.to_string()),
        },
        "set_password" => {
            if let Err(e) = require_reauth_for_credential_change(&ctx, &me, form.reauth.trim(), locale).await {
                fail!(e);
            }
            match domain::user::set_password(&ctx, &me.username, non_empty(form.password.clone()).as_deref(), locale)
                .await
            {
                Ok(cas) => cas,
                Err(e) => fail!(e.to_string()),
            }
        }
        "enable_totp" => match domain::user::enable_totp(&ctx, &me.username, locale).await {
            Ok((cas, secret)) => {
                pending_totp_secret = Some(secret);
                cas
            }
            Err(e) => fail!(e.to_string()),
        },
        "confirm_totp" => match domain::user::confirm_totp(&ctx, &me.username, form.totp_code.trim(), locale).await {
            Ok((cas, codes)) => {
                revealed_recovery_codes = Some(codes);
                cas
            }
            Err(e) => fail!(e.to_string()),
        },
        "disable_totp" => {
            if let Err(e) = require_reauth_throttled(&ctx, &me, form.totp_code.trim(), locale).await {
                fail!(e);
            }
            match domain::user::disable_totp(&ctx, &me.username, locale).await {
                Ok(cas) => cas,
                Err(e) => fail!(e.to_string()),
            }
        }
        "regenerate_recovery_codes" => {
            if let Err(e) = require_reauth_throttled(&ctx, &me, form.totp_code.trim(), locale).await {
                fail!(e);
            }
            match domain::user::regenerate_recovery_codes(&ctx, &me.username, locale).await {
                Ok((cas, codes)) => {
                    revealed_recovery_codes = Some(codes);
                    cas
                }
                Err(e) => fail!(e.to_string()),
            }
        }
        other => fail!(gitquarry_i18n::shared::unknown_action(locale, other)),
    };
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let flash = match cas {
        Cas::Ok => Flash::Ok("saved."),
        Cas::Conflict => Flash::Err(&cas_conflict_msg),
    };
    // Re-fetch: the actions above already wrote the change, but this
    // handler still needs the full, current `User` record to render below.
    let (Some(me), _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username).await? else {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::account_no_longer_exists(locale)
        )));
    };
    if headers.contains_key(crate::webui::ASYNC_HEADER) && form.action == "remove_pubkey" {
        return Ok(
            axum::response::Html(render_ssh_keys_table(&me, "/_account/security", &csrf, locale).into_string())
                .into_response(),
        );
    }
    if headers.contains_key(crate::webui::ASYNC_HEADER) && form.action == "remove_gpg_key" {
        return Ok(
            axum::response::Html(render_gpg_keys_table(&me, "/_account/security", &csrf, locale).into_string())
                .into_response(),
        );
    }
    if headers.contains_key(crate::webui::ASYNC_HEADER) && form.action == "remove_secondary_email" {
        return Ok(axum::response::Html(
            render_secondary_emails_table(&me, "/_account/security", &csrf, locale).into_string(),
        )
        .into_response());
    }
    let content = render_security_page(
        &me,
        &csrf,
        Some(flash),
        pending_totp_secret.as_deref(),
        revealed_recovery_codes.as_deref(),
        locale,
    );
    let body = account_settings_page("security", content, federation_ui_active(&ctx), locale).into_string();
    Ok(axum::response::Html(layout(
        "security",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// The pubkey table alone, wrapped in a stable `id` so `remove_pubkey` can
/// swap just this fragment (`data-async-target="#ssh-keys-table"`, see
/// `crate::webui::ASYNC_HEADER`'s own doc comment) instead of re-rendering
/// the whole page.
fn render_ssh_keys_table(user: &User, action_url: &str, csrf: &str, locale: gitquarry_i18n::Locale) -> Markup {
    use crate::auth::i18n::security as t;
    html! {
        div id="ssh-keys-table" {
        table class="list" {
            tr { th { (t::pubkey_column_header(locale)) } th {} }
            @if user.pubkeys.is_empty() {
                tr { td colspan="2" { em { (t::no_keys_yet_hint(locale)) } } }
            }
            @for pubkey in &user.pubkeys {
                @let fp = access::key_fingerprint(pubkey).unwrap_or_else(|_| "?".to_string());
                tr {
                    td { code { (fp) } }
                    td {
                        form method="post" action=(action_url) style="display:inline" data-async data-async-target="#ssh-keys-table" {
                            (PreEscaped(csrf))
                            input type="hidden" name="security_action" value="remove_pubkey";
                            input type="hidden" name="fingerprint" value=(fp);
                            button type="submit" { (t::delete_button(locale)) }
                        }
                    }
                }
            }
        }
        }
    }
}

/// The secondary-emails table alone, wrapped in a stable `id` — same
/// swap-just-this-fragment shape [`render_ssh_keys_table`] uses.
fn render_secondary_emails_table(user: &User, action_url: &str, csrf: &str, locale: gitquarry_i18n::Locale) -> Markup {
    use crate::auth::i18n::security as t;
    html! {
        div id="secondary-emails-table" {
        table class="list" {
            tr { th { (t::secondary_email_column_header(locale)) } th {} }
            @if user.secondary_emails.is_empty() {
                tr { td colspan="2" { em { (t::no_secondary_emails_yet_hint(locale)) } } }
            }
            @for email in &user.secondary_emails {
                tr {
                    td { (email) }
                    td {
                        form method="post" action=(action_url) style="display:inline" data-async data-async-target="#secondary-emails-table" {
                            (PreEscaped(csrf))
                            input type="hidden" name="security_action" value="remove_secondary_email";
                            input type="hidden" name="secondary_email" value=(email);
                            button type="submit" { (t::delete_button(locale)) }
                        }
                    }
                }
            }
        }
        }
    }
}

/// The GPG key table alone, wrapped in a stable `id` — same
/// swap-just-this-fragment shape [`render_ssh_keys_table`] uses. Unlike
/// that one, the fingerprint shown here is already stored on
/// `GpgKeyEntry` (see its own doc comment): nothing here calls into
/// `crate::gpg` at all, this is a plain sync render like every other view.
fn render_gpg_keys_table(user: &User, action_url: &str, csrf: &str, locale: gitquarry_i18n::Locale) -> Markup {
    use crate::auth::i18n::security as t;
    html! {
        div id="gpg-keys-table" {
        table class="list" {
            tr { th { (t::gpg_key_column_header(locale)) } th {} }
            @if user.gpg_keys.is_empty() {
                tr { td colspan="2" { em { (t::no_gpg_keys_yet_hint(locale)) } } }
            }
            @for key in &user.gpg_keys {
                tr {
                    td { code { (key.fingerprint) } }
                    td {
                        form method="post" action=(action_url) style="display:inline" data-async data-async-target="#gpg-keys-table" {
                            (PreEscaped(csrf))
                            input type="hidden" name="security_action" value="remove_gpg_key";
                            input type="hidden" name="gpg_fingerprint" value=(key.fingerprint);
                            button type="submit" { (t::delete_button(locale)) }
                        }
                    }
                }
            }
        }
        }
    }
}

pub fn render_security_page(
    user: &User,
    csrf: &str,
    message: Option<Flash>,
    pending_totp_secret: Option<&str>,
    revealed_recovery_codes: Option<&[String]>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use crate::auth::i18n::security as t;
    let action_url = "/_account/security";
    html! {
        p class="muted" {
            (t::logged_in_as(locale)) strong { (user.username) }
            @if let Some(email) = &user.email {
                " " (format!("<{email}>"))
            }
            @if user.is_admin {
                " · " span class="badge accent" { (t::admin_badge(locale)) }
            }
            (t::managed_elsewhere_prefix(locale)) code { "admin" } (t::managed_elsewhere_suffix(locale))
            a href="/_users" { "/_users" }
            (t::own_keys_and_password_suffix(locale))
        }
        (PreEscaped(render_flash(message, locale)))

        h4 { (t::ssh_keys_heading(locale)) }
        (render_ssh_keys_table(user, action_url, csrf, locale))
        form method="post" action=(action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="security_action" value="add_pubkey";
            input name="pubkey" size="60" placeholder=(t::add_pubkey_placeholder(locale));
            @if user.password_hash.is_some() || user.totp_secret.is_some() {
                input type="password" name="reauth" size="30" placeholder=(t::reauth_placeholder(locale));
            }
            button type="submit" { (t::add_key_button(locale)) }
        }

        h4 { (t::secondary_emails_heading(locale)) }
        p class="muted" { (t::secondary_emails_purpose(locale)) }
        (render_secondary_emails_table(user, action_url, csrf, locale))
        form method="post" action=(action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="security_action" value="add_secondary_email";
            input name="secondary_email" size="40" placeholder=(t::add_secondary_email_placeholder(locale));
            button type="submit" { (t::add_secondary_email_button(locale)) }
        }

        h4 { (t::gpg_keys_heading(locale)) }
        p class="muted" { (t::gpg_keys_purpose(locale)) }
        (render_gpg_keys_table(user, action_url, csrf, locale))
        form method="post" action=(action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="security_action" value="add_gpg_key";
            textarea name="gpg_key" rows="6" cols="64" placeholder=(t::add_gpg_key_placeholder(locale)) {}
            p { button type="submit" { (t::add_gpg_key_button(locale)) } }
        }

        h4 { (t::password_heading(locale)) }
        p class="muted" { (t::password_purpose_prefix(locale)) code { "/_login" } (t::password_purpose_suffix(locale)) }
        form method="post" action=(action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="security_action" value="set_password";
            input type="password" name="password" size="30"
                placeholder=(if user.password_hash.is_some() { t::password_no_changes_placeholder(locale) } else { t::password_not_set_placeholder(locale) });
            @if user.password_hash.is_some() || user.totp_secret.is_some() {
                input type="password" name="reauth" size="30" placeholder=(t::reauth_placeholder(locale));
            }
            button type="submit" {
                (if user.password_hash.is_some() { t::change_password_button(locale) } else { t::set_password_button(locale) })
            }
        }

        h4 { (t::two_factor_heading(locale)) }
        @if user.totp_enabled {
            p class="muted" { (t::totp_active_hint(locale)) }
            form method="post" action=(action_url) {
                (PreEscaped(csrf))
                input type="hidden" name="security_action" value="disable_totp";
                input name="totp_code" size="24" placeholder=(t::totp_disable_placeholder(locale));
                button type="submit" { (t::disable_totp_button(locale)) }
            }

            h4 { (t::recovery_codes_heading(locale)) }
            @if let Some(codes) = revealed_recovery_codes {
                div class="alert alert-warn" { span class="alert-body" { (t::recovery_codes_reveal_warning(locale)) } }
                pre { @for code in codes { (code) "\n" } }
            }
            p class="muted" { (t::recovery_codes_remaining_hint(locale, user.totp_recovery_codes.len())) }
            form method="post" action=(action_url) {
                (PreEscaped(csrf))
                input type="hidden" name="security_action" value="regenerate_recovery_codes";
                input name="totp_code" size="24" placeholder=(t::regenerate_recovery_codes_placeholder(locale));
                button type="submit" { (t::regenerate_recovery_codes_button(locale)) }
            }
        } @else if let Some(secret) = pending_totp_secret {
            @let uri = totp::otpauth_uri("gitquarry", &user.username, secret);
            p class="muted" { (t::totp_scan_hint(locale)) }
            @if let Some(svg) = totp::qr_svg(&uri) {
                div class="totp-qr" { (PreEscaped(svg)) }
            }
            p { code { (secret) } }
            p { a href=(uri) { (t::totp_open_app_link(locale)) } }
            form method="post" action=(action_url) {
                (PreEscaped(csrf))
                input type="hidden" name="security_action" value="confirm_totp";
                input name="totp_code" size="10" placeholder=(t::totp_confirm_placeholder(locale)) autofocus;
                button type="submit" { (t::confirm_totp_button(locale)) }
            }
        } @else {
            p class="muted" { (t::totp_inactive_hint(locale)) }
            form method="post" action=(action_url) {
                (PreEscaped(csrf))
                input type="hidden" name="security_action" value="enable_totp";
                button type="submit" { (t::enable_totp_button(locale)) }
            }
        }
    }
}
