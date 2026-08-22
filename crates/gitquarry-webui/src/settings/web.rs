//! `.conf/global.json`-backed form: server settings (network binds, mirrors
//! cache, OIDC). [`require_admin`] is the shared admin-page gate — also used
//! by [`crate::auth::web::users`] and [`crate::webui::repo_settings`], which
//! is why it lives here rather than in either of those.

use crate::auth::web::login;
use crate::webui::{
    AppError, Flash, State_, admin_header, csrf_field, effective_density, effective_locale, effective_theme, layout,
    non_empty, render_flash, require_csrf, settings_section,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::ServerSettings;
use maud::{Markup, PreEscaped, html};

/// `None` if this request may proceed to render an admin page
/// (`/_settings`, `/_users`, a repo's "settings" tab); `Some(response)` is
/// what to return instead. When no login method is configured at all (no
/// OIDC and no user has a password), this always returns `None` — admin
/// pages stay exactly as open as they've always been. That "is anything
/// configured" question goes through [`login::login_enforced`], not
/// `password_login_available`: a backend error there must not read as
/// "nothing configured" and skip this gate entirely. Otherwise: no session
/// at all redirects to `/_login`; a real session for a non-admin user gets
/// a 403 rather than looping back to a login page that can't help them.
pub(crate) async fn require_admin(ctx: &AppContext, headers: &HeaderMap, return_to: &str) -> Option<Response> {
    let login_configured = login::login_enforced(ctx).await;
    if !login_configured {
        // First boot: no session can exist yet, so the only thing that can
        // stand in for one is the token this process printed at startup,
        // already exchanged for a grant cookie at `/_bootstrap`.
        if crate::webui::bootstrap::holds_grant(ctx, headers) {
            return None;
        }
        let locale = effective_locale(ctx, headers).await;
        return Some(
            AppError::forbidden(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::bootstrap_grant_required(locale)
            ))
            .into_response(),
        );
    }
    match login::current_user(ctx, headers).await {
        Some(user) if user.is_admin => None,
        Some(_) => {
            let locale = effective_locale(ctx, headers).await;
            Some(
                AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::account_is_not_admin(locale)
                ))
                .into_response(),
            )
        }
        None => Some(login_redirect(return_to)),
    }
}

fn login_redirect(return_to: &str) -> Response {
    Redirect::to(&format!("/_login?return_to={}", crate::webui::url_encode(return_to))).into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct SettingsForm {
    web_bind: String,
    ssh_bind: String,
    mirrors_dir: String,
    mirrors_max_age: String,
    mirrors_max_size: String,
    #[serde(default)]
    oidc_issuer_url: String,
    #[serde(default)]
    oidc_client_id: String,
    #[serde(default)]
    oidc_client_secret: String,
    #[serde(default)]
    oidc_redirect_base_url: String,
    /// One host per line or comma-separated — see [`parse_mirror_hosts`].
    /// Extra hosts only: the forges with a built-in client are always
    /// allowed and aren't in this field.
    #[serde(default)]
    mirror_extra_hosts: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_settings_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings").await {
        return Ok(resp);
    }
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let body = settings_body(&settings, &csrf_field(&headers), None, locale).into_string();
    Ok(axum::response::Html(layout(
        "settings",
        &admin_header("settings", locale),
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

pub(crate) async fn handler_settings_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<SettingsForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let mirrors_max_age = non_empty(form.mirrors_max_age);
    let mirrors_max_size = non_empty(form.mirrors_max_size);
    // Read once, for two things: resolving the write-only client secret
    // just below, and the CAS etag at the write. One read rather than two —
    // `request_cache_middleware` would serve the second from this same
    // request's cache anyway, so a "re-read right before writing" wouldn't
    // actually be fresher. The CAS still does its job: this etag is from
    // this request, so a concurrent edit in between conflicts rather than
    // getting stomped.
    let (current, etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let oidc_issuer_url = non_empty(form.oidc_issuer_url);
    let oidc_client_id = non_empty(form.oidc_client_id);
    let oidc_redirect_base_url = non_empty(form.oidc_redirect_base_url);
    // Write-only field: `settings_body` never echoes the stored
    // secret back, since `type="password"` hides it from the screen but not
    // from the page source, where it would sit in plain text for anything
    // that reads the DOM or caches the response. So a blank submission
    // means "leave it as it is" — the same shape the external-mirror token
    // field already uses. The one exception is blanking it alongside the
    // rest of the OIDC block, which reads as "turn OIDC off" and clears the
    // secret too rather than stranding it against the all-4-or-none rule
    // below.
    let oidc_still_configured =
        oidc_issuer_url.is_some() || oidc_client_id.is_some() || oidc_redirect_base_url.is_some();
    let oidc_client_secret = match non_empty(form.oidc_client_secret) {
        Some(submitted) => Some(submitted),
        None if oidc_still_configured => current.oidc_client_secret.clone(),
        None => None,
    };
    let oidc_fields = (
        oidc_issuer_url,
        oidc_client_id,
        oidc_client_secret,
        oidc_redirect_base_url,
    );

    // Caught here instead of only at the next restart (where
    // `mirror::gc::parse_*`/`oidc::discover` are what actually consume
    // these): a typo shouldn't need a server bounce to surface as an
    // error, and shouldn't lose the rest of the form to a separate error
    // page either — a rejected value re-renders this same page with
    // everything else still filled in, same as the CAS-conflict case below.
    let mirror_extra_hosts = parse_mirror_hosts(&form.mirror_extra_hosts);
    let bad_mirror_host = mirror_extra_hosts.iter().find(|host| !is_host_pattern(host)).cloned();
    let validation_error = if let Some(host) = bad_mirror_host {
        Some(gitquarry_i18n::shared::invalid_mirror_host_pattern(locale, &host))
    } else if let Some(e) = mirrors_max_age
        .as_deref()
        .and_then(|age| gitquarry_core::mirror::gc::parse_age(age).err())
    {
        Some(e.to_string())
    } else if let Some(e) = mirrors_max_size
        .as_deref()
        .and_then(|size| gitquarry_core::mirror::gc::parse_size(size).err())
    {
        Some(e.to_string())
    } else if !matches!(
        oidc_fields,
        (Some(_), Some(_), Some(_), Some(_)) | (None, None, None, None)
    ) {
        // Same "all 4 or none" rule `gitquarry_auth_crypto::oidc::discover` enforces at startup.
        Some(gitquarry_i18n::shared::oidc_fields_go_together(locale))
    } else {
        None
    };

    let settings = ServerSettings {
        web_bind: non_empty(form.web_bind),
        ssh_bind: non_empty(form.ssh_bind),
        mirrors_dir: non_empty(form.mirrors_dir),
        mirrors_max_age,
        mirrors_max_size,
        oidc_issuer_url: oidc_fields.0,
        oidc_client_id: oidc_fields.1,
        oidc_client_secret: oidc_fields.2,
        oidc_redirect_base_url: oidc_fields.3,
        mirror_extra_hosts: mirror_extra_hosts.clone(),
    };
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let flash = if let Some(msg) = &validation_error {
        Flash::Err(msg)
    } else {
        match storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher).await? {
            Cas::Ok => Flash::Ok("saved to the bucket. Restart the server to apply."),
            Cas::Conflict => Flash::Err(&cas_conflict_msg),
        }
    };
    let body = settings_body(&settings, &csrf_field(&headers), Some(flash), locale);
    // Async (`data-async-target="#settings-body"`, see `crate::webui::ASYNC_HEADER`'s
    // own doc comment): a JS client gets just this fragment back, its
    // `<form data-async>` never actually navigates, so there's nothing for
    // a later F5 to "resubmit" a warning about — the browser's address bar
    // and history entry stay on the original GET the whole time. The full
    // page below is still the fallback a plain (non-JS) submit gets, same
    // "still works without it" contract every other `data-async` form here
    // already has.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        return Ok(axum::response::Html(body.into_string()).into_response());
    }
    let topnav = login::topnav(&ctx, &headers).await;
    Ok(axum::response::Html(layout(
        "settings",
        &admin_header("settings", locale),
        &body.into_string(),
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Splits the textarea into host patterns — newline *or* comma separated,
/// since an operator pasting a list will use either, and lowercased because
/// a host comparison that depends on how it was typed isn't a comparison.
fn parse_mirror_hosts(raw: &str) -> Vec<String> {
    raw.split(['\n', ','])
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// A host name, or a `*.` prefix followed by one. ASCII only: an IDN typed
/// in unicode can't be compared safely against what a URL carries, so it has
/// to be given in punycode.
fn is_host_pattern(pattern: &str) -> bool {
    let host = pattern.strip_prefix("*.").unwrap_or(pattern);
    !host.is_empty()
        && host != "."
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

/// `#settings-body`'s id is what `data-async-target` on the `<form>` below
/// names — the flash banner sits *outside* the form in the markup (above
/// it), so a plain `data-async` (whose default target is the form itself)
/// would swap the form back in without ever refreshing that banner. Wrapping
/// both in one element and targeting that instead is what keeps a "saved" /
/// error message showing up after an async submit, not just a full-page one.
#[cfg(any(test, feature = "test-util"))]
pub fn settings_body(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    settings_body_impl(settings, csrf, message, locale)
}
#[cfg(not(any(test, feature = "test-util")))]
fn settings_body(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    settings_body_impl(settings, csrf, message, locale)
}
fn settings_body_impl(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    fn field(v: &Option<String>) -> &str {
        v.as_deref().unwrap_or("")
    }
    use crate::settings::i18n as t;
    html! {
      div id="settings-body" {
        p class="muted" {
            (t::settings_saved_in_bucket_prefix(locale)) code { ".conf/global.json" } (t::settings_saved_in_bucket_suffix(locale))
        }
        (PreEscaped(render_flash(message, locale)))
        form method="post" action="/_settings" data-async data-async-target="#settings-body" {
            (PreEscaped(csrf))
            (settings_section(&t::settings_network_heading(locale), None, html! {
                div class="field" {
                    label { (t::settings_web_bind_label(locale)) }
                    input name="web_bind" value=(field(&settings.web_bind)) placeholder="127.0.0.1:8080";
                }
                div class="field" {
                    label { (t::settings_ssh_bind_label(locale)) }
                    input name="ssh_bind" value=(field(&settings.ssh_bind)) placeholder="127.0.0.1:2222";
                }
            }))
            (settings_section(&t::settings_mirrors_cache_heading(locale), None, html! {
                div class="field" {
                    label { (t::settings_mirrors_dir_label(locale)) }
                    input name="mirrors_dir" value=(field(&settings.mirrors_dir))
                        placeholder=(t::settings_mirrors_dir_placeholder(locale)) size="50";
                }
                div class="field" {
                    label { (t::settings_mirrors_max_age_label(locale)) }
                    input name="mirrors_max_age" value=(field(&settings.mirrors_max_age)) placeholder=(t::settings_mirrors_max_age_placeholder(locale));
                }
                div class="field" {
                    label { (t::settings_mirrors_max_size_label(locale)) }
                    input name="mirrors_max_size" value=(field(&settings.mirrors_max_size)) placeholder=(t::settings_mirrors_max_size_placeholder(locale));
                }
            }))
            (settings_section(&t::settings_external_mirrors_heading(locale), Some(&t::settings_mirror_hosts_prose(locale)), html! {
                p class="muted" {
                    (t::settings_mirror_hosts_builtin_prefix(locale))
                    code { (gitquarry_mirror_external::validate::BUILTIN_HOSTS.join(", ")) }
                    (t::settings_mirror_hosts_builtin_suffix(locale))
                }
                div class="field" {
                    label { (t::settings_mirror_hosts_label(locale)) }
                    textarea name="mirror_extra_hosts" rows="4" cols="50"
                        placeholder="gitlab.tudominio.com\n*.gitea.interno" { (settings.mirror_extra_hosts.join("\n")) }
                }
            }))
            (settings_section(&t::settings_oidc_heading(locale), None, html! {
                p class="muted" {
                    (t::settings_oidc_intro_gates(locale)) code { "/_settings" } ", " code { "/_users" } ", "
                    (t::settings_oidc_intro_tab(locale))
                    a href="/_users" { "/_users" } (t::settings_oidc_intro_see(locale)) code { "private" } (t::settings_oidc_intro_private(locale))
                    a href="/_users" { "/_users" } (t::settings_oidc_intro_see_there(locale))
                }
                div class="field" {
                    label { (t::settings_issuer_url_label(locale)) }
                    input name="oidc_issuer_url" value=(field(&settings.oidc_issuer_url)) placeholder="https://idp.example.com" size="50";
                }
                div class="field" {
                    label { (t::settings_client_id_label(locale)) }
                    input name="oidc_client_id" value=(field(&settings.oidc_client_id)) size="50";
                }
                div class="field" {
                    label { (t::settings_client_secret_label(locale)) }
                    // No `value`: see `handler_settings_post` on why this one
                    // field is write-only. Blank means "unchanged".
                    input type="password" name="oidc_client_secret" size="50"
                        placeholder=(if settings.oidc_client_secret.is_some() { t::settings_client_secret_placeholder_unchanged(locale) } else { t::settings_client_secret_placeholder_unset(locale) });
                }
                div class="field" {
                    label { (t::settings_redirect_base_url_label(locale)) }
                    input name="oidc_redirect_base_url" value=(field(&settings.oidc_redirect_base_url))
                        placeholder=(t::settings_redirect_base_url_placeholder(locale)) size="50";
                }
            }))
            p { button type="submit" { (t::settings_save_button(locale)) } }
        }
      }
    }
}
