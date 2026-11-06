//! `/_account/appearance`: self-service display defaults (text-size, theme,
//! density) for whoever is logged in — split out of [`super::security`]
//! (identity: SSH keys, password) on purpose, same reasoning as
//! [`super::favorites`] splitting off the starred-repos list: a display
//! preference isn't an account-identity setting. The session-only overrides
//! (the "A" buttons on a repo's summary tab, the ☀/☾ buttons in the topbar)
//! still live where they're used — this page only sets what a fresh session
//! with no override yet falls back to (see [`crate::webui::effective_text_size`]
//! and [`crate::webui::effective_theme`]).
//!
//! No username in the URL — always the logged-in session's own account, see
//! `crate::webui::account_settings_page`'s own doc comment.

use super::login;
use crate::webui::{
    AppError, Flash, State_, TextSize, account_settings_page, csrf_field, effective_density, effective_locale,
    effective_theme, federation_ui_active, layout, render_flash, require_csrf, require_login, settings_section,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::User;
use gitquarry_core::storage::Cas;
use maud::{Markup, PreEscaped, html};

pub(crate) async fn handler_appearance_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let user = match require_login(&ctx, &headers, "/_account/appearance").await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_appearance_page(&user, &csrf_field(&headers), None, locale);
    let body = account_settings_page("appearance", content, federation_ui_active(&ctx), locale).into_string();
    Ok(axum::response::Html(layout(
        "appearance",
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
pub(crate) struct AppearanceForm {
    /// Not `action` on the wire: a control named `action` shadows the
    /// `data-async` form's own `.action` property, so `style/app.js` would
    /// post to `[object HTMLInputElement]` (see `webui::actions`).
    #[serde(rename = "appearance_action")]
    action: String,
    #[serde(default)]
    csrf: String,
    /// Only read by `set_text_size_pref`: `"sm"`/`"md"`/`"lg"`.
    #[serde(default)]
    text_size: String,
    /// Only read by `set_theme_pref`: `"dark"`/`"light"`, or empty for
    /// "no preference, follow the system".
    #[serde(default)]
    theme: String,
    /// Only read by `set_density_pref`: `"dense"`/`"airy"`, or empty for
    /// "no preference, use the site default (dense)".
    #[serde(default)]
    density: String,
    /// Only read by `set_locale_pref`: `"es"`/`"en"` (`gitquarry_i18n::Locale::parse`'s
    /// accepted set), or empty for "no preference, use the site default".
    #[serde(default)]
    locale: String,
}

pub(crate) async fn handler_appearance_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<AppearanceForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/appearance").await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;

    // Re-read the etag right before writing so we don't stomp a concurrent
    // edit: the CAS fails and we report it instead of overwriting. One
    // small user object now, not the whole bucket's worth of users.
    let (Some(mut me), etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username).await? else {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::auth::account_no_longer_exists(locale)
        )));
    };

    match form.action.as_str() {
        "set_text_size_pref" => {
            TextSize::parse(&form.text_size).ok_or_else(|| {
                AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::invalid_size(locale, &form.text_size)
                ))
            })?;
            me.text_size_default = form.text_size.clone();
        }
        "set_theme_pref" => {
            if !matches!(form.theme.as_str(), "" | "dark" | "light") {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::invalid_theme(locale, &form.theme)
                )));
            }
            me.theme_default = form.theme.clone();
        }
        "set_density_pref" => {
            if !matches!(form.density.as_str(), "" | "dense" | "airy") {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::invalid_density(locale, &form.density)
                )));
            }
            me.density_default = form.density.clone();
        }
        "set_locale_pref" => {
            if !matches!(form.locale.as_str(), "" | "es" | "en") {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::invalid_locale(locale, &form.locale)
                )));
            }
            me.locale_default = form.locale.clone();
        }
        other => {
            return Err(AppError::bad_request(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::unknown_action(locale, other)
            )));
        }
    }

    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &me, etag.as_deref()).await?;
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let flash = match cas {
        Cas::Ok => Flash::Ok("saved."),
        Cas::Conflict => Flash::Err(&cas_conflict_msg),
    };
    let content = render_appearance_page(&me, &csrf_field(&headers), Some(flash), locale);
    // Async (`data-async-target="#appearance-body"`, see `crate::webui::ASYNC_HEADER`'s
    // own doc comment): a JS client's `<form data-async>` never actually
    // navigates, so there's no POST result for a later F5 to warn about
    // resubmitting.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    let topnav = login::topnav(&ctx, &headers).await;
    let body = account_settings_page("appearance", content, federation_ui_active(&ctx), locale).into_string();
    Ok(axum::response::Html(layout(
        "appearance",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// All 4 forms below share one `#appearance-body` async target instead of
/// each owning its own: the flash banner sits above all of them (one save
/// confirms, or rejects, one preference at a time — there's no "which form"
/// for it to belong to individually), so any of the 4 submits has to
/// refresh the whole wrapper to get a fresh banner, same reasoning
/// `settings::web::settings_body`'s own doc comment gives for `/_settings`.
pub(crate) fn render_appearance_page(
    user: &User,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let current = TextSize::parse(&user.text_size_default);
    let action_url = "/_account/appearance";
    html! {
      div id="appearance-body" {
        (PreEscaped(render_flash(message, locale)))

        (settings_section(&gitquarry_i18n::web::auth::appearance::text_size_heading(locale), Some(&gitquarry_i18n::web::auth::appearance::text_size_description(locale)), html! {
            form method="post" action=(action_url) data-async data-async-target="#appearance-body" {
                (PreEscaped(csrf))
                input type="hidden" name="appearance_action" value="set_text_size_pref";
                select name="text_size" {
                    option value="sm" selected[current == Some(TextSize::Sm)] { (gitquarry_i18n::web::auth::appearance::text_size_small(locale)) }
                    option value="md" selected[current == Some(TextSize::Md)] { (gitquarry_i18n::web::auth::appearance::text_size_medium(locale)) }
                    option value="lg" selected[current == Some(TextSize::Lg)] { (gitquarry_i18n::web::auth::appearance::text_size_large(locale)) }
                }
                button type="submit" { (gitquarry_i18n::web::auth::appearance::save_button(locale)) }
            }
        }))

        (settings_section(&gitquarry_i18n::web::auth::appearance::theme_heading(locale), Some(&gitquarry_i18n::web::auth::appearance::theme_description(locale)), html! {
            form method="post" action=(action_url) data-async data-async-target="#appearance-body" {
                (PreEscaped(csrf))
                input type="hidden" name="appearance_action" value="set_theme_pref";
                select name="theme" {
                    option value="" selected[user.theme_default.is_empty()] { (gitquarry_i18n::web::auth::appearance::theme_automatic(locale)) }
                    option value="light" selected[user.theme_default == "light"] { (gitquarry_i18n::web::auth::appearance::theme_light(locale)) }
                    option value="dark" selected[user.theme_default == "dark"] { (gitquarry_i18n::web::auth::appearance::theme_dark(locale)) }
                }
                button type="submit" { (gitquarry_i18n::web::auth::appearance::save_button(locale)) }
            }
        }))

        (settings_section(&gitquarry_i18n::web::auth::appearance::density_heading(locale), Some(&gitquarry_i18n::web::auth::appearance::density_description(locale)), html! {
            form method="post" action=(action_url) data-async data-async-target="#appearance-body" {
                (PreEscaped(csrf))
                input type="hidden" name="appearance_action" value="set_density_pref";
                select name="density" {
                    option value="" selected[user.density_default.is_empty()] { (gitquarry_i18n::web::auth::appearance::density_default_label(locale)) }
                    option value="dense" selected[user.density_default == "dense"] { (gitquarry_i18n::web::auth::appearance::density_dense_label(locale)) }
                    option value="airy" selected[user.density_default == "airy"] { (gitquarry_i18n::web::auth::appearance::density_airy_label(locale)) }
                }
                button type="submit" { (gitquarry_i18n::web::auth::appearance::save_button(locale)) }
            }
        }))

        (settings_section(&gitquarry_i18n::web::auth::appearance::locale_heading(locale), Some(&gitquarry_i18n::web::auth::appearance::locale_description(locale)), html! {
            form method="post" action=(action_url) data-async data-async-target="#appearance-body" {
                (PreEscaped(csrf))
                input type="hidden" name="appearance_action" value="set_locale_pref";
                select name="locale" {
                    option value="" selected[user.locale_default.is_empty()] { (gitquarry_i18n::web::auth::appearance::locale_automatic(locale)) }
                    option value="es" selected[user.locale_default == "es"] { (gitquarry_i18n::web::auth::appearance::locale_es(locale)) }
                    option value="en" selected[user.locale_default == "en"] { (gitquarry_i18n::web::auth::appearance::locale_en(locale)) }
                }
                button type="submit" { (gitquarry_i18n::web::auth::appearance::save_button(locale)) }
            }
        }))
      }
    }
}
