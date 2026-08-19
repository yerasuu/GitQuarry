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
    effective_theme, layout, render_flash, require_csrf, require_login,
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
        Err(resp) => return Ok(resp),
    };
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_appearance_page(&user, &csrf_field(&headers), None, locale);
    let body = account_settings_page("appearance", content, locale).into_string();
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
}

pub(crate) async fn handler_appearance_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<AppearanceForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/appearance").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;

    // Re-read the etag right before writing so we don't stomp a concurrent
    // edit: the CAS fails and we report it instead of overwriting. One
    // small user object now, not the whole bucket's worth of users.
    let (Some(mut me), etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username).await? else {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::account_no_longer_exists(locale)
        )));
    };

    match form.action.as_str() {
        "set_text_size_pref" => {
            TextSize::parse(&form.text_size).ok_or_else(|| {
                AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::invalid_size(locale, &form.text_size)
                ))
            })?;
            me.text_size_default = form.text_size.clone();
        }
        "set_theme_pref" => {
            if !matches!(form.theme.as_str(), "" | "dark" | "light") {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::invalid_theme(locale, &form.theme)
                )));
            }
            me.theme_default = form.theme.clone();
        }
        "set_density_pref" => {
            if !matches!(form.density.as_str(), "" | "dense" | "airy") {
                return Err(AppError::bad_request(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::invalid_density(locale, &form.density)
                )));
            }
            me.density_default = form.density.clone();
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
    let topnav = login::topnav(&ctx, &headers).await;
    let content = render_appearance_page(&me, &csrf_field(&headers), Some(flash), locale);
    let body = account_settings_page("appearance", content, locale).into_string();
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

pub fn render_appearance_page(
    user: &User,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let current = TextSize::parse(&user.text_size_default);
    let action_url = "/_account/appearance";
    html! {
        (PreEscaped(render_flash(message, locale)))

        h4 { (crate::auth::i18n::appearance::text_size_heading(locale)) }
        p class="muted" { (crate::auth::i18n::appearance::text_size_description(locale)) }
        form method="post" action=(action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="action" value="set_text_size_pref";
            select name="text_size" {
                option value="sm" selected[current == Some(TextSize::Sm)] { (crate::auth::i18n::appearance::text_size_small(locale)) }
                option value="md" selected[current == Some(TextSize::Md)] { (crate::auth::i18n::appearance::text_size_medium(locale)) }
                option value="lg" selected[current == Some(TextSize::Lg)] { (crate::auth::i18n::appearance::text_size_large(locale)) }
            }
            button type="submit" { (crate::auth::i18n::appearance::save_button(locale)) }
        }

        h4 { (crate::auth::i18n::appearance::theme_heading(locale)) }
        p class="muted" { (crate::auth::i18n::appearance::theme_description(locale)) }
        form method="post" action=(action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="action" value="set_theme_pref";
            select name="theme" {
                option value="" selected[user.theme_default.is_empty()] { (crate::auth::i18n::appearance::theme_automatic(locale)) }
                option value="light" selected[user.theme_default == "light"] { (crate::auth::i18n::appearance::theme_light(locale)) }
                option value="dark" selected[user.theme_default == "dark"] { (crate::auth::i18n::appearance::theme_dark(locale)) }
            }
            button type="submit" { (crate::auth::i18n::appearance::save_button(locale)) }
        }

        h4 { (crate::auth::i18n::appearance::density_heading(locale)) }
        p class="muted" { (crate::auth::i18n::appearance::density_description(locale)) }
        form method="post" action=(action_url) {
            (PreEscaped(csrf))
            input type="hidden" name="action" value="set_density_pref";
            select name="density" {
                option value="" selected[user.density_default.is_empty()] { (crate::auth::i18n::appearance::density_default_label(locale)) }
                option value="dense" selected[user.density_default == "dense"] { (crate::auth::i18n::appearance::density_dense_label(locale)) }
                option value="airy" selected[user.density_default == "airy"] { (crate::auth::i18n::appearance::density_airy_label(locale)) }
            }
            button type="submit" { (crate::auth::i18n::appearance::save_button(locale)) }
        }
    }
}
