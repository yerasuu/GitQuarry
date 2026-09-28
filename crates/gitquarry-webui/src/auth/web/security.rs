//! `/_account/security`: how whoever is logged in *signs in* — password,
//! two-factor, and the recovery codes that get past a lost authenticator.
//! Nothing else: keys moved to [`super::keys`] (they authenticate a push or
//! vouch for a commit, they are not a login), email addresses to
//! [`super::avatar`]'s profile section (an address is an identity, not a
//! credential), and display defaults were already at
//! [`super::appearance`]. [`super::users`] is the admin-only equivalent
//! that can edit *any* account's.
//!
//! No username in the URL — always the logged-in session's own account,
//! resolved server-side via [`login::current_user`], same as every other
//! `/_account/*` section (see `crate::webui::account_settings_page`'s own
//! doc comment). `is_admin` isn't editable here: it stays an admin-only
//! grant, same as always.

use super::{login, reauth};
use crate::webui::{
    AppError, Flash, State_, account_settings_page, csrf_field, effective_density, effective_locale, effective_theme,
    federation_ui_active, field, layout, non_empty, render_flash, require_csrf, require_login, settings_section,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use gitquarry_auth_crypto::totp;
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
    password: String,
    /// Second copy of `password`, typed again. A password field shows
    /// nothing back, so without this a typo silently becomes the new
    /// password — and for an account with no other way in (no SSH key, no
    /// OIDC) that is a lockout with no recovery path.
    #[serde(default)]
    password_confirm: String,
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
        ($msg:expr) => {{
            let content = render_security_page(&me, &csrf, Some(Flash::Err(&$msg)), None, None, locale);
            if headers.contains_key(crate::webui::ASYNC_HEADER) {
                return Ok(axum::response::Html(content.into_string()).into_response());
            }
            return Ok(axum::response::Html(layout(
                "security",
                "",
                &account_settings_page("security", content, federation_ui_active(&ctx), locale).into_string(),
                &topnav,
                effective_theme(&ctx, &headers).await,
                effective_density(&ctx, &headers).await,
                locale,
            ))
            .into_response());
        }};
    }

    let mut pending_totp_secret = None;
    let mut revealed_recovery_codes = None;
    let cas = match form.action.as_str() {
        "set_password" => {
            if let Err(e) = reauth::require_reauth_for_credential_change(&ctx, &me, form.reauth.trim(), locale).await {
                fail!(e);
            }
            if form.password != form.password_confirm {
                fail!(gitquarry_i18n::web::auth::security::password_mismatch(locale));
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
            if let Err(e) = reauth::require_reauth_throttled(&ctx, &me, form.totp_code.trim(), locale).await {
                fail!(e);
            }
            match domain::user::disable_totp(&ctx, &me.username, locale).await {
                Ok(cas) => cas,
                Err(e) => fail!(e.to_string()),
            }
        }
        "regenerate_recovery_codes" => {
            if let Err(e) = reauth::require_reauth_throttled(&ctx, &me, form.totp_code.trim(), locale).await {
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
            gitquarry_i18n::shared::auth::account_no_longer_exists(locale)
        )));
    };
    let content = render_security_page(
        &me,
        &csrf,
        Some(flash),
        pending_totp_secret.as_deref(),
        revealed_recovery_codes.as_deref(),
        locale,
    );
    // Async (`data-async-target="#security-body"`, see `crate::webui::ASYNC_HEADER`'s
    // own doc comment) — a revealed TOTP secret/QR or recovery-code set
    // still shows up fine in the swapped fragment, it just never triggers
    // a real page navigation for an F5 to later warn about resubmitting.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
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

/// All 5 forms below (only 1-3 render at once, depending on 2FA state)
/// target this one `#security-body` — every action can reveal something
/// only the *next* render shows (a TOTP secret + QR, a set of recovery
/// codes) or change which of the three 2FA forms even exists, so a submit
/// has to refresh the whole wrapper, not just the form that fired it. Same
/// reasoning `render_appearance_page`'s own doc comment gives.
pub(crate) fn render_security_page(
    user: &User,
    csrf: &str,
    message: Option<Flash>,
    pending_totp_secret: Option<&str>,
    revealed_recovery_codes: Option<&[String]>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::auth::security as t;
    let action_url = "/_account/security";
    // Only asked for where there is something to re-prove: an account with
    // neither a password nor a TOTP secret (an OIDC-only user setting a
    // first password) has no second factor, and demanding one would lock it
    // out of ever acquiring one.
    let needs_reauth = user.password_hash.is_some() || user.totp_secret.is_some();
    let has_password = user.password_hash.is_some();
    html! {
      div id="security-body" {
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

        (settings_section(&t::password_heading(locale), None, html! {
            p class="muted" { (t::password_purpose_prefix(locale)) code { "/_login" } (t::password_purpose_suffix(locale)) }
            form method="post" action=(action_url) data-async data-async-target="#security-body" data-confirm=[has_password.then(|| t::change_password_confirm(locale))] {
                (PreEscaped(csrf))
                input type="hidden" name="security_action" value="set_password";
                // `username` in a hidden field, and `autocomplete` on every
                // control: without them a password manager cannot tell which
                // account this changes, nor which box is the new password —
                // it offers to fill the current one into all three.
                input type="hidden" name="username" value=(user.username) autocomplete="username";
                (field(&t::new_password_label(locale), Some(&t::clear_password_hint(locale)), html! {
                    input type="password" name="password" size="30" autocomplete="new-password";
                }))
                (field(&t::confirm_password_label(locale), None, html! {
                    input type="password" name="password_confirm" size="30" autocomplete="new-password";
                }))
                @if needs_reauth {
                    (field(&t::reauth_label(locale), Some(&t::reauth_hint(locale)), html! {
                        input type="password" name="reauth" size="30" autocomplete="current-password";
                    }))
                }
                button type="submit" {
                    (if has_password { t::change_password_button(locale) } else { t::set_password_button(locale) })
                }
            }
        }))

        (settings_section(&t::two_factor_heading(locale), None, html! {
            @if user.totp_enabled {
                p class="muted" { (t::totp_active_hint(locale)) }
                form method="post" action=(action_url) data-async data-async-target="#security-body" data-confirm=(t::disable_totp_confirm(locale)) {
                    (PreEscaped(csrf))
                    input type="hidden" name="security_action" value="disable_totp";
                    (field(&t::reauth_label(locale), Some(&t::reauth_hint(locale)), html! {
                        input name="totp_code" size="24" placeholder=(t::totp_disable_placeholder(locale)) autocomplete="one-time-code";
                    }))
                    button type="submit" { (t::disable_totp_button(locale)) }
                }
            } @else if let Some(secret) = pending_totp_secret {
                @let uri = totp::otpauth_uri("gitquarry", &user.username, secret);
                p class="muted" { (t::totp_scan_hint(locale)) }
                @if let Some(svg) = totp::qr_svg(&uri) {
                    div class="totp-qr" { (PreEscaped(svg)) }
                }
                p { code { (secret) } }
                p { a href=(uri) { (t::totp_open_app_link(locale)) } }
                form method="post" action=(action_url) data-async data-async-target="#security-body" {
                    (PreEscaped(csrf))
                    input type="hidden" name="security_action" value="confirm_totp";
                    (field(&t::totp_confirm_placeholder(locale), None, html! {
                        input name="totp_code" size="10" autocomplete="one-time-code" autofocus;
                    }))
                    button type="submit" { (t::confirm_totp_button(locale)) }
                }
            } @else {
                p class="muted" { (t::totp_inactive_hint(locale)) }
                form method="post" action=(action_url) data-async data-async-target="#security-body" {
                    (PreEscaped(csrf))
                    input type="hidden" name="security_action" value="enable_totp";
                    button type="submit" { (t::enable_totp_button(locale)) }
                }
            }
        }))

        // Only once 2FA is actually on: recovery codes exist to get past a
        // lost authenticator, so there is nothing to show — or regenerate —
        // before there is an authenticator to lose.
        @if user.totp_enabled {
            (settings_section(&t::recovery_codes_heading(locale), None, html! {
                @if let Some(codes) = revealed_recovery_codes {
                    div class="alert alert-warn" { span class="alert-body" { (t::recovery_codes_reveal_warning(locale)) } }
                    pre { @for code in codes { (code) "\n" } }
                }
                p class="muted" { (t::recovery_codes_remaining_hint(locale, user.totp_recovery_codes.len())) }
                form method="post" action=(action_url) data-async data-async-target="#security-body" data-confirm=(t::regenerate_recovery_codes_confirm(locale)) {
                    (PreEscaped(csrf))
                    input type="hidden" name="security_action" value="regenerate_recovery_codes";
                    (field(&t::reauth_label(locale), Some(&t::reauth_hint(locale)), html! {
                        input name="totp_code" size="24" placeholder=(t::regenerate_recovery_codes_placeholder(locale)) autocomplete="one-time-code";
                    }))
                    button type="submit" { (t::regenerate_recovery_codes_button(locale)) }
                }
            }))
        }
      }
    }
}
