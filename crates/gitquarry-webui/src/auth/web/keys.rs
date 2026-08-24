//! `/_account/keys`: the logged-in account's own SSH and GPG keys.
//!
//! Split out of [`super::security`], which had grown into six unrelated
//! blocks on one flat page. These two belong together and apart from the
//! rest: an SSH key is a *transport* credential (it authenticates a
//! `git push`) and a GPG key is an *attestation* one (it vouches for
//! commits already pushed) — neither is a way to log in, which is what
//! `security` is now exclusively about (password, 2FA, recovery codes).
//!
//! Copy comes from [`gitquarry_i18n::web::auth::security`] rather than a catalog of
//! its own: the strings didn't change meaning by moving page, and forking
//! them would mean two ids for one sentence.
//!
//! No username in the URL — always the logged-in session's own account,
//! same as every other `/_account/*` section (see
//! `crate::webui::account_settings_page`'s own doc comment).

use super::{login, reauth};
use crate::webui::{
    AppError, Flash, State_, account_settings_page, csrf_field, effective_density, effective_locale, effective_theme,
    field, layout, render_flash, require_csrf, require_login, settings_section,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::User;
use gitquarry_core::storage::Cas;
use maud::{Markup, PreEscaped, html};

const ACTION_URL: &str = "/_account/keys";

pub(crate) async fn handler_keys_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, ACTION_URL).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let csrf = csrf_field(&headers);
    let locale = effective_locale(&ctx, &headers).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let body = account_settings_page("keys", render_keys_page(&me, &csrf, None, locale), locale).into_string();
    Ok(axum::response::Html(layout(
        "keys",
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
pub(crate) struct KeysForm {
    /// Wire name `keys_action`, not `action`, for the same reason
    /// [`super::security::SecurityForm`] renames its own: the remove forms
    /// are `data-async`, and `HTMLFormElement` is `[OverrideBuiltins]`, so a
    /// control literally named `action` shadows the form's own `.action`
    /// IDL property and `style/app.js` would post to the `<input>` element
    /// instead of the URL.
    #[serde(rename = "keys_action")]
    action: String,
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    pubkey: String,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    gpg_key: String,
    #[serde(default)]
    gpg_fingerprint: String,
    /// Current password or live TOTP code, proving the session holder is
    /// the account holder — see
    /// [`reauth::require_reauth_for_credential_change`]. Only the SSH side
    /// asks for it: an SSH key authenticates future pushes on its own, so a
    /// stolen cookie must not be able to plant one. A GPG key grants no
    /// access at all — it only makes already-pushed commits show as
    /// verified — so gating it would cost friction for nothing.
    #[serde(default)]
    reauth: String,
}

pub(crate) async fn handler_keys_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<KeysForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, ACTION_URL).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let csrf = csrf_field(&headers);

    // A rejected action re-renders this same page with `Flash::Err` rather
    // than a generic error page — same reasoning `security`'s own handler
    // gives: losing the whole key list over one bad field is a worse
    // outcome than showing the error where it can be retried. Async is
    // different: the add/remove targets below are the narrow per-table
    // `id`s (`#ssh-keys-table`/`#gpg-keys-table`), which never contain a
    // flash banner (`render_keys_page`'s own shell has it, outside either
    // table) — swapping a *page* fragment into one of those would corrupt
    // it (`style/app.js`'s `target.replaceWith` takes the response's first
    // element, which would be the whole `<html>`), so this goes through
    // `AppError` instead: a real non-2xx status, picked up by
    // `extractFlashMessage`/`showErrorToast` the same way any other failed
    // `data-async` request already is.
    macro_rules! fail {
        ($msg:expr) => {{
            if headers.contains_key(crate::webui::ASYNC_HEADER) {
                return Err(AppError::bad_request(anyhow::anyhow!("{}", $msg)));
            }
            return Ok(axum::response::Html(layout(
                "keys",
                "",
                &account_settings_page(
                    "keys",
                    render_keys_page(&me, &csrf, Some(Flash::Err(&$msg)), locale),
                    locale,
                )
                .into_string(),
                &topnav,
                effective_theme(&ctx, &headers).await,
                effective_density(&ctx, &headers).await,
                locale,
            ))
            .into_response());
        }};
    }

    let cas = match form.action.as_str() {
        "add_pubkey" => {
            if let Err(e) = reauth::require_reauth_for_credential_change(&ctx, &me, form.reauth.trim(), locale).await {
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
        "add_gpg_key" => match domain::user::add_gpg_key(&ctx, &me.username, form.gpg_key.trim(), locale).await {
            Ok(cas) => cas,
            Err(e) => fail!(e.to_string()),
        },
        "remove_gpg_key" => match domain::user::remove_gpg_key(&ctx, &me.username, &form.gpg_fingerprint, locale).await
        {
            Ok(cas) => cas,
            Err(e) => fail!(e.to_string()),
        },
        _ => Cas::Ok,
    };
    if cas == Cas::Conflict {
        fail!(gitquarry_i18n::shared::cas_conflict(locale));
    }

    let Some(me) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username)
        .await?
        .0
    else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::user_not_found(locale, &me.username)
        )));
    };

    // Async (`data-async-target`, see `webui::ASYNC_HEADER`'s own doc
    // comment): an add or a remove only ever changes its own table, so swap
    // just that fragment instead of re-rendering the page around it — the
    // page-level "saved." flash below is skipped on this path the same way
    // a removal already skipped it, not shown at all rather than shown
    // somewhere a table-only swap can't reach.
    if headers.contains_key(crate::webui::ASYNC_HEADER)
        && matches!(form.action.as_str(), "add_pubkey" | "remove_pubkey")
    {
        return Ok(axum::response::Html(render_ssh_keys_table(&me, &csrf, locale).into_string()).into_response());
    }
    if headers.contains_key(crate::webui::ASYNC_HEADER)
        && matches!(form.action.as_str(), "add_gpg_key" | "remove_gpg_key")
    {
        return Ok(axum::response::Html(render_gpg_keys_table(&me, &csrf, locale).into_string()).into_response());
    }

    let body = account_settings_page(
        "keys",
        render_keys_page(&me, &csrf, Some(Flash::Ok("saved.")), locale),
        locale,
    )
    .into_string();
    Ok(axum::response::Html(layout(
        "keys",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// The SSH key table alone, wrapped in a stable `id` so a removal can swap
/// just this fragment instead of the whole page.
fn render_ssh_keys_table(user: &User, csrf: &str, locale: gitquarry_i18n::Locale) -> Markup {
    use gitquarry_i18n::web::auth::security as t;
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
                            form method="post" action=(ACTION_URL) class="inline-form" data-async data-async-target="#ssh-keys-table" data-confirm=(t::remove_ssh_key_confirm(locale, &fp)) {
                                (PreEscaped(csrf))
                                input type="hidden" name="keys_action" value="remove_pubkey";
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

/// The GPG key table alone, same swap-just-this-fragment shape
/// [`render_ssh_keys_table`] uses. Unlike that one, the fingerprint shown
/// is already stored on `GpgKeyEntry` (see its own doc comment): nothing
/// here calls into `crate::gpg` at all, this is a plain sync render.
fn render_gpg_keys_table(user: &User, csrf: &str, locale: gitquarry_i18n::Locale) -> Markup {
    use gitquarry_i18n::web::auth::security as t;
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
                            form method="post" action=(ACTION_URL) class="inline-form" data-async data-async-target="#gpg-keys-table" data-confirm=(t::remove_gpg_key_confirm(locale, &key.fingerprint)) {
                                (PreEscaped(csrf))
                                input type="hidden" name="keys_action" value="remove_gpg_key";
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

#[cfg(any(test, feature = "test-util"))]
pub fn render_keys_page(user: &User, csrf: &str, message: Option<Flash>, locale: gitquarry_i18n::Locale) -> Markup {
    render_keys_page_impl(user, csrf, message, locale)
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_keys_page(user: &User, csrf: &str, message: Option<Flash>, locale: gitquarry_i18n::Locale) -> Markup {
    render_keys_page_impl(user, csrf, message, locale)
}

fn render_keys_page_impl(user: &User, csrf: &str, message: Option<Flash>, locale: gitquarry_i18n::Locale) -> Markup {
    use gitquarry_i18n::web::auth::security as t;
    // Only asked for where there is something to re-prove: an account with
    // neither a password nor a TOTP secret has no second factor, and
    // demanding one would lock it out of ever acquiring a key.
    let needs_reauth = user.password_hash.is_some() || user.totp_secret.is_some();
    html! {
        (PreEscaped(render_flash(message, locale)))

        (settings_section(&t::ssh_keys_heading(locale), None, html! {
            (render_ssh_keys_table(user, csrf, locale))
            form method="post" action=(ACTION_URL) data-async data-async-target="#ssh-keys-table" {
                (PreEscaped(csrf))
                input type="hidden" name="keys_action" value="add_pubkey";
                (field(&t::add_pubkey_label(locale), Some(&t::add_pubkey_hint(locale)), html! {
                    input name="pubkey" size="60" placeholder=(t::add_pubkey_placeholder(locale));
                }))
                @if needs_reauth {
                    (field(&t::reauth_label(locale), Some(&t::reauth_hint(locale)), html! {
                        input type="password" name="reauth" size="30" autocomplete="current-password";
                    }))
                }
                button type="submit" { (t::add_key_button(locale)) }
            }
        }))

        (settings_section(&t::gpg_keys_heading(locale), Some(&t::gpg_keys_purpose(locale)), html! {
            (render_gpg_keys_table(user, csrf, locale))
            form method="post" action=(ACTION_URL) data-async data-async-target="#gpg-keys-table" {
                (PreEscaped(csrf))
                input type="hidden" name="keys_action" value="add_gpg_key";
                (field(&t::add_gpg_key_label(locale), None, html! {
                    textarea name="gpg_key" rows="6" cols="64" placeholder=(t::add_gpg_key_placeholder(locale)) {}
                }))
                button type="submit" { (t::add_gpg_key_button(locale)) }
            }
        }))
    }
}
