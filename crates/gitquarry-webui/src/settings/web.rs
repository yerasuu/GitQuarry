//! `.conf/global.json`-backed server settings (network binds, mirrors cache,
//! external-mirror allowlist, OIDC, backup) — one real page per concern under
//! `/_settings/<section>` (`footer`/`network`/`mirrors-cache`/
//! `external-mirrors`/`oidc`/`backup`), same shape [`crate::webui::account_settings_page`]
//! already gives `/_account/*`: a save's CAS/validation stays scoped to the
//! section that actually submitted, instead of one giant form covering
//! every field at once. [`require_admin`] is the shared admin-page gate —
//! also used by [`crate::auth::web::users`] and [`crate::webui::repo_settings`],
//! which is why it lives here rather than in either of those.

use crate::auth::web::login;
use crate::webui::repo::relative_time_ago;
use crate::webui::{
    AppError, Flash, State_, admin_header, csrf_field, effective_density, effective_locale, effective_theme,
    federation_ui_active, layout, non_empty, render_flash, require_csrf, settings_section,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::app::AppContext;
use gitquarry_core::backup::dest::Dest;
use gitquarry_core::backup::inventory::Selection;
use gitquarry_core::backup::report::{self, BackupRun, Outcome, Trigger};
use gitquarry_core::backup::schedule;
use gitquarry_core::storage;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::ServerSettings;
use gitquarry_i18n::web::chrome;
use maud::{Markup, PreEscaped, html};
use std::sync::Arc;
use tracing::warn;

/// `None` if this request may proceed to render an admin page
/// (`/_settings/*`, `/_users`, a repo's "settings" tab); `Some(response)` is
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
                gitquarry_i18n::shared::settings::bootstrap_grant_required(locale)
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
                    gitquarry_i18n::shared::auth::account_is_not_admin(locale)
                ))
                .into_response(),
            )
        }
        None => Some(login_redirect(return_to)),
    }
}

/// Same shape as [`require_admin`], but also lets a non-admin
/// `is_federation_moderator` user through — the gate `/_peers` uses
/// (`docs/federation-binary-split-concept.md` section 6: managing linked
/// peers is a narrower delegation than full `is_admin`).
pub(crate) async fn require_admin_or_federation_moderator(
    ctx: &AppContext,
    headers: &HeaderMap,
    return_to: &str,
) -> Option<Response> {
    let login_configured = login::login_enforced(ctx).await;
    if !login_configured {
        return None;
    }
    match login::current_user(ctx, headers).await {
        Some(user) if user.is_admin || user.is_federation_moderator => None,
        Some(_) => {
            let locale = effective_locale(ctx, headers).await;
            Some(
                AppError::forbidden(anyhow::anyhow!(
                    "{}",
                    gitquarry_i18n::shared::auth::account_is_not_admin_or_moderator(locale)
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

/// `(id, href, label)` for [`settings_page`]'s sidebar — each a real
/// `/_settings/<id>` route rather than an anchor into one page: unlike the
/// old single-form layout, a save here is CAS/validated against just the
/// section it belongs to, so there's no reason left to keep them on one
/// page the way [`crate::webui::repo_settings::settings_sections`]'s
/// `?view=` split still has to (that split predates per-section routes in
/// this app and hasn't been revisited yet).
fn settings_sections(locale: gitquarry_i18n::Locale) -> [(&'static str, &'static str, String); 6] {
    use gitquarry_i18n::web::settings as t;
    [
        ("footer", "/_settings/footer", t::settings_footer_heading(locale)),
        ("network", "/_settings/network", t::settings_network_heading(locale)),
        (
            "mirrors-cache",
            "/_settings/mirrors-cache",
            t::settings_mirrors_cache_heading(locale),
        ),
        (
            "external-mirrors",
            "/_settings/external-mirrors",
            t::settings_external_mirrors_heading(locale),
        ),
        ("oidc", "/_settings/oidc", t::settings_oidc_heading(locale)),
        ("backup", "/_settings/backup", t::settings_backup_heading(locale)),
    ]
}

/// Same sidebar-plus-content shell [`crate::webui::account_settings_page`]
/// gives `/_account/*` — `content` is one section's own body, this wraps it
/// with the sidebar naming every section and highlighting `active`.
fn settings_page(active: &str, content: Markup, locale: gitquarry_i18n::Locale) -> Markup {
    let sections = settings_sections(locale);
    let title = sections
        .iter()
        .find(|(id, _, _)| *id == active)
        .map(|(_, _, label)| label.clone())
        .unwrap_or_default();
    html! {
        div class="settings-layout" {
            nav class="settings-sidebar" aria-label=(chrome::server_settings_sidebar_nav_label(locale)) {
                ul {
                    @for (id, href, label) in &sections {
                        li {
                            a class=[(active == *id).then_some("active")] aria-current=[(active == *id).then_some("page")] href=(href) { (label) }
                        }
                    }
                }
            }
            div class="settings-content" {
                h2 class="settings-title" { (title) }
                (content)
            }
        }
    }
}

fn field(v: &Option<String>) -> &str {
    v.as_deref().unwrap_or("")
}

/// `p.muted` above every section's own form, same "where does this go"
/// prose the old single-page layout showed once.
fn saved_in_bucket_prose(locale: gitquarry_i18n::Locale) -> Markup {
    use gitquarry_i18n::web::settings as t;
    html! {
        p class="muted" {
            (t::settings_saved_in_bucket_prefix(locale)) code { ".conf/global.json" } (t::settings_saved_in_bucket_suffix(locale))
        }
    }
}

async fn render_page(
    ctx: &AppContext,
    headers: &HeaderMap,
    active: &'static str,
    content: Markup,
    locale: gitquarry_i18n::Locale,
) -> Response {
    let topnav = login::topnav(ctx, headers).await;
    let body = settings_page(active, content, locale);
    axum::response::Html(layout(
        &format!("settings-{active}"),
        &admin_header("settings", federation_ui_active(ctx), locale),
        &body.into_string(),
        &topnav,
        effective_theme(ctx, headers).await,
        effective_density(ctx, headers).await,
        locale,
    ))
    .into_response()
}

/// Async (`data-async-target="#<section>-body"`, see
/// `crate::webui::ASYNC_HEADER`'s own doc comment): a JS client's
/// `<form data-async>` never actually navigates, so there's no POST result
/// for a later F5 to warn about resubmitting.
fn is_async(headers: &HeaderMap) -> bool {
    headers.contains_key(crate::webui::ASYNC_HEADER)
}

// ---------------------------------------------------------------- footer --

#[derive(serde::Deserialize)]
pub(crate) struct FooterForm {
    #[serde(default)]
    source_link: String,
    #[serde(default)]
    csrf: String,
}

fn render_footer_page(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::settings as t;
    html! {
        div id="footer-body" {
            (saved_in_bucket_prose(locale))
            (PreEscaped(render_flash(message, locale)))
            (settings_section(&t::settings_footer_heading(locale), Some(&t::settings_source_link_prose(locale)), html! {
                form method="post" action="/_settings/footer" data-async data-async-target="#footer-body" {
                    (PreEscaped(csrf))
                    div class="field" {
                        label { (t::settings_source_link_label(locale)) }
                        input name="source_link" value=(field(&settings.source_link))
                            placeholder="namespace/name" size="50";
                    }
                    p { button type="submit" { (t::settings_save_button(locale)) } }
                }
            }))
        }
    }
}

pub(crate) async fn handler_settings_footer_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/footer").await {
        return Ok(resp);
    }
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_footer_page(&settings, &csrf_field(&headers), None, locale);
    Ok(render_page(&ctx, &headers, "footer", content, locale).await)
}

pub(crate) async fn handler_settings_footer_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<FooterForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/footer").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (current, etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let settings = ServerSettings {
        source_link: non_empty(form.source_link),
        ..current
    };
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let flash =
        match storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher).await? {
            Cas::Ok => Flash::Ok("saved to the bucket. Restart the server to apply."),
            Cas::Conflict => Flash::Err(&cas_conflict_msg),
        };
    let content = render_footer_page(&settings, &csrf_field(&headers), Some(flash), locale);
    if is_async(&headers) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    Ok(render_page(&ctx, &headers, "footer", content, locale).await)
}

// --------------------------------------------------------------- network --

#[derive(serde::Deserialize)]
pub(crate) struct NetworkForm {
    #[serde(default)]
    web_bind: String,
    #[serde(default)]
    ssh_bind: String,
    #[serde(default)]
    csrf: String,
}

fn render_network_page(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::settings as t;
    html! {
        div id="network-body" {
            (saved_in_bucket_prose(locale))
            (PreEscaped(render_flash(message, locale)))
            (settings_section(&t::settings_network_heading(locale), None, html! {
                form method="post" action="/_settings/network" data-async data-async-target="#network-body" {
                    (PreEscaped(csrf))
                    div class="field" {
                        label { (t::settings_web_bind_label(locale)) }
                        input name="web_bind" value=(field(&settings.web_bind)) placeholder="127.0.0.1:8080";
                    }
                    div class="field" {
                        label { (t::settings_ssh_bind_label(locale)) }
                        input name="ssh_bind" value=(field(&settings.ssh_bind)) placeholder="127.0.0.1:2222";
                    }
                    p { button type="submit" { (t::settings_save_button(locale)) } }
                }
            }))
        }
    }
}

pub(crate) async fn handler_settings_network_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/network").await {
        return Ok(resp);
    }
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_network_page(&settings, &csrf_field(&headers), None, locale);
    Ok(render_page(&ctx, &headers, "network", content, locale).await)
}

pub(crate) async fn handler_settings_network_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<NetworkForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/network").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (current, etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let settings = ServerSettings {
        web_bind: non_empty(form.web_bind),
        ssh_bind: non_empty(form.ssh_bind),
        ..current
    };
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let flash =
        match storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher).await? {
            Cas::Ok => Flash::Ok("saved to the bucket. Restart the server to apply."),
            Cas::Conflict => Flash::Err(&cas_conflict_msg),
        };
    let content = render_network_page(&settings, &csrf_field(&headers), Some(flash), locale);
    if is_async(&headers) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    Ok(render_page(&ctx, &headers, "network", content, locale).await)
}

// ---------------------------------------------------------- mirrors-cache --

#[derive(serde::Deserialize)]
pub(crate) struct MirrorsCacheForm {
    #[serde(default)]
    mirrors_dir: String,
    #[serde(default)]
    mirrors_max_age: String,
    #[serde(default)]
    mirrors_max_size: String,
    #[serde(default)]
    mirrors_large_repo_threshold: String,
    #[serde(default)]
    csrf: String,
}

fn render_mirrors_cache_page(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::settings as t;
    html! {
        div id="mirrors-cache-body" {
            (saved_in_bucket_prose(locale))
            (PreEscaped(render_flash(message, locale)))
            (settings_section(&t::settings_mirrors_cache_heading(locale), None, html! {
                form method="post" action="/_settings/mirrors-cache" data-async data-async-target="#mirrors-cache-body" {
                    (PreEscaped(csrf))
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
                    div class="field" {
                        label { (t::settings_mirrors_large_repo_threshold_label(locale)) }
                        input name="mirrors_large_repo_threshold" value=(field(&settings.mirrors_large_repo_threshold))
                            placeholder=(t::settings_mirrors_large_repo_threshold_placeholder(locale));
                    }
                    p { button type="submit" { (t::settings_save_button(locale)) } }
                }
            }))
        }
    }
}

pub(crate) async fn handler_settings_mirrors_cache_get(
    State(ctx): State_,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/mirrors-cache").await {
        return Ok(resp);
    }
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_mirrors_cache_page(&settings, &csrf_field(&headers), None, locale);
    Ok(render_page(&ctx, &headers, "mirrors-cache", content, locale).await)
}

pub(crate) async fn handler_settings_mirrors_cache_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<MirrorsCacheForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/mirrors-cache").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (current, etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let mirrors_max_age = non_empty(form.mirrors_max_age);
    let mirrors_max_size = non_empty(form.mirrors_max_size);
    let mirrors_large_repo_threshold = non_empty(form.mirrors_large_repo_threshold);
    let validation_error = mirrors_max_age
        .as_deref()
        .and_then(|age| gitquarry_core::mirror::gc::parse_age(age).err())
        .or_else(|| {
            mirrors_max_size
                .as_deref()
                .and_then(|size| gitquarry_core::mirror::gc::parse_size(size).err())
        })
        .or_else(|| {
            mirrors_large_repo_threshold
                .as_deref()
                .and_then(|size| gitquarry_core::mirror::gc::parse_size(size).err())
        })
        .map(|e| e.to_string());
    let settings = ServerSettings {
        mirrors_dir: non_empty(form.mirrors_dir),
        mirrors_max_age,
        mirrors_max_size,
        mirrors_large_repo_threshold,
        ..current
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
    let content = render_mirrors_cache_page(&settings, &csrf_field(&headers), Some(flash), locale);
    if is_async(&headers) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    Ok(render_page(&ctx, &headers, "mirrors-cache", content, locale).await)
}

// ------------------------------------------------------- external-mirrors --

#[derive(serde::Deserialize)]
pub(crate) struct ExternalMirrorsForm {
    /// One host per line or comma-separated — see [`parse_mirror_hosts`].
    /// Extra hosts only: the forges with a built-in client are always
    /// allowed and aren't in this field.
    #[serde(default)]
    mirror_extra_hosts: String,
    #[serde(default)]
    csrf: String,
}

fn render_external_mirrors_page(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::settings as t;
    html! {
        div id="external-mirrors-body" {
            (saved_in_bucket_prose(locale))
            (PreEscaped(render_flash(message, locale)))
            (settings_section(&t::settings_external_mirrors_heading(locale), Some(&t::settings_mirror_hosts_prose(locale)), html! {
                p class="muted" {
                    (t::settings_mirror_hosts_builtin_prefix(locale))
                    code { (gitquarry_mirror_external::validate::BUILTIN_HOSTS.join(", ")) }
                    (t::settings_mirror_hosts_builtin_suffix(locale))
                }
                form method="post" action="/_settings/external-mirrors" data-async data-async-target="#external-mirrors-body" {
                    (PreEscaped(csrf))
                    div class="field" {
                        label { (t::settings_mirror_hosts_label(locale)) }
                        textarea name="mirror_extra_hosts" rows="4" cols="50"
                            placeholder="gitlab.tudominio.com\n*.gitea.interno" { (settings.mirror_extra_hosts.join("\n")) }
                    }
                    p { button type="submit" { (t::settings_save_button(locale)) } }
                }
            }))
        }
    }
}

pub(crate) async fn handler_settings_external_mirrors_get(
    State(ctx): State_,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/external-mirrors").await {
        return Ok(resp);
    }
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_external_mirrors_page(&settings, &csrf_field(&headers), None, locale);
    Ok(render_page(&ctx, &headers, "external-mirrors", content, locale).await)
}

pub(crate) async fn handler_settings_external_mirrors_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<ExternalMirrorsForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/external-mirrors").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (current, etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    // Caught here instead of only at the next restart (where
    // `gitquarry_mirror_external`'s own host check is what actually
    // consumes this): a typo shouldn't need a server bounce to surface as
    // an error, and shouldn't lose the rest of the form to a separate error
    // page either — a rejected value re-renders this same page with
    // everything else still filled in.
    let mirror_extra_hosts = parse_mirror_hosts(&form.mirror_extra_hosts);
    let bad_mirror_host = mirror_extra_hosts.iter().find(|host| !is_host_pattern(host)).cloned();
    let validation_error =
        bad_mirror_host.map(|host| gitquarry_i18n::shared::settings::invalid_mirror_host_pattern(locale, &host));
    let settings = ServerSettings {
        mirror_extra_hosts: mirror_extra_hosts.clone(),
        ..current
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
    let content = render_external_mirrors_page(&settings, &csrf_field(&headers), Some(flash), locale);
    if is_async(&headers) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    Ok(render_page(&ctx, &headers, "external-mirrors", content, locale).await)
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

// ------------------------------------------------------------------ oidc --

#[derive(serde::Deserialize)]
pub(crate) struct OidcForm {
    #[serde(default)]
    oidc_issuer_url: String,
    #[serde(default)]
    oidc_client_id: String,
    #[serde(default)]
    oidc_client_secret: String,
    #[serde(default)]
    oidc_redirect_base_url: String,
    #[serde(default)]
    csrf: String,
}

fn render_oidc_page(
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::settings as t;
    html! {
        div id="oidc-body" {
            (saved_in_bucket_prose(locale))
            (PreEscaped(render_flash(message, locale)))
            (settings_section(&t::settings_oidc_heading(locale), None, html! {
                p class="muted" {
                    (t::settings_oidc_intro_gates(locale)) code { "/_settings" } ", " code { "/_users" } ", "
                    (t::settings_oidc_intro_tab(locale))
                    a href="/_users" { "/_users" } (t::settings_oidc_intro_see(locale)) code { "private" } (t::settings_oidc_intro_private(locale))
                    a href="/_users" { "/_users" } (t::settings_oidc_intro_see_there(locale))
                }
                form method="post" action="/_settings/oidc" data-async data-async-target="#oidc-body" data-confirm=(t::settings_oidc_save_confirm(locale)) {
                    (PreEscaped(csrf))
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
                        // No `value`: see `handler_settings_oidc_post` on why this one
                        // field is write-only. Blank means "unchanged".
                        input type="password" name="oidc_client_secret" size="50"
                            placeholder=(if settings.oidc_client_secret.is_some() { t::settings_client_secret_placeholder_unchanged(locale) } else { t::settings_client_secret_placeholder_unset(locale) });
                    }
                    div class="field" {
                        label { (t::settings_redirect_base_url_label(locale)) }
                        input name="oidc_redirect_base_url" value=(field(&settings.oidc_redirect_base_url))
                            placeholder=(t::settings_redirect_base_url_placeholder(locale)) size="50";
                    }
                    p { button type="submit" { (t::settings_save_button(locale)) } }
                }
            }))
        }
    }
}

pub(crate) async fn handler_settings_oidc_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/oidc").await {
        return Ok(resp);
    }
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_oidc_page(&settings, &csrf_field(&headers), None, locale);
    Ok(render_page(&ctx, &headers, "oidc", content, locale).await)
}

pub(crate) async fn handler_settings_oidc_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<OidcForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/oidc").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (current, etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let oidc_issuer_url = non_empty(form.oidc_issuer_url);
    let oidc_client_id = non_empty(form.oidc_client_id);
    let oidc_redirect_base_url = non_empty(form.oidc_redirect_base_url);
    // Write-only field: `render_oidc_page` never echoes the stored secret
    // back, since `type="password"` hides it from the screen but not from
    // the page source, where it would sit in plain text for anything that
    // reads the DOM or caches the response. So a blank submission means
    // "leave it as it is" — the same shape the external-mirror token field
    // already uses. The one exception is blanking it alongside the rest of
    // the OIDC block, which reads as "turn OIDC off" and clears the secret
    // too rather than stranding it against the all-4-or-none rule below.
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
    // Same "all 4 or none" rule `gitquarry_auth_crypto::oidc::discover` enforces at startup.
    let validation_error = if !matches!(
        oidc_fields,
        (Some(_), Some(_), Some(_), Some(_)) | (None, None, None, None)
    ) {
        Some(gitquarry_i18n::shared::settings::oidc_fields_go_together(locale))
    } else {
        None
    };
    let settings = ServerSettings {
        oidc_issuer_url: oidc_fields.0,
        oidc_client_id: oidc_fields.1,
        oidc_client_secret: oidc_fields.2,
        oidc_redirect_base_url: oidc_fields.3,
        ..current
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
    let content = render_oidc_page(&settings, &csrf_field(&headers), Some(flash), locale);
    if is_async(&headers) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    Ok(render_page(&ctx, &headers, "oidc", content, locale).await)
}

// ---------------------------------------------------------------- backup --

#[derive(serde::Deserialize)]
pub(crate) struct BackupForm {
    #[serde(default)]
    backup_dest: String,
    // Unchecked checkboxes aren't submitted at all, so every toggle here is
    // an `Option`: `Some` means checked, `None` means the box was cleared.
    #[serde(default)]
    enabled: Option<String>,
    #[serde(default)]
    interval: String,
    #[serde(default)]
    snapshot: Option<String>,
    #[serde(default)]
    class_git: Option<String>,
    #[serde(default)]
    class_lfs: Option<String>,
    #[serde(default)]
    class_releases: Option<String>,
    #[serde(default)]
    class_avatars: Option<String>,
    #[serde(default)]
    class_entities: Option<String>,
    #[serde(default)]
    class_audit: Option<String>,
    #[serde(default)]
    class_derived: Option<String>,
    #[serde(default)]
    keep: String,
    #[serde(default)]
    audit_keep_months: String,
    #[serde(default)]
    delete_grace_days: String,
    #[serde(default)]
    concurrency: String,
    #[serde(default)]
    csrf: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct BackupRunForm {
    #[serde(default)]
    csrf: String,
}

fn checked(field: &Option<String>) -> bool {
    field.is_some()
}

fn num_field(v: Option<u32>) -> String {
    v.map(|n| n.to_string()).unwrap_or_default()
}

/// Empty means "unset", which every one of these fields treats as its own
/// documented default — so a blank box is a real answer here, not a
/// validation error.
fn parse_optional_u32(raw: &str, what: &str) -> Result<Option<u32>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<u32>()
        .map(Some)
        .map_err(|_| format!("{what}: not a number"))
}

/// Byte counts in a run report span "a few JSON objects" to "a night of
/// packs", so they're rendered in whichever unit keeps them readable.
fn bytes_human(n: u64) -> String {
    const UNITS: [(u64, &str); 4] = [(1 << 30, "G"), (1 << 20, "M"), (1 << 10, "K"), (1, "B")];
    for (scale, suffix) in UNITS {
        if n >= scale {
            // One decimal below 10 of a unit, none above: "1.4G" is useful,
            // "941.7M" is noise.
            let value = n as f64 / scale as f64;
            return if value < 10.0 && scale > 1 {
                format!("{value:.1}{suffix}")
            } else {
                format!("{}{suffix}", value.round() as u64)
            };
        }
    }
    "0B".to_string()
}

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "running",
        Outcome::Ok => "ok",
        Outcome::Partial => "partial",
        Outcome::Failed => "failed",
    }
}

fn trigger_label(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Scheduled => "scheduled",
        Trigger::Manual => "manual",
        Trigger::Cli => "cli",
    }
}

fn render_backup_page(
    settings: &ServerSettings,
    runs: &[BackupRun],
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::settings as t;
    let selection = Selection::from_settings(settings);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    html! {
        div id="backup-body" {
            (saved_in_bucket_prose(locale))
            (PreEscaped(render_flash(message, locale)))
            (settings_section(&t::settings_backup_heading(locale), Some(&t::settings_backup_prose(locale)), html! {
                form method="post" action="/_settings/backup" data-async data-async-target="#backup-body" {
                    (PreEscaped(csrf))
                    div class="field" {
                        label { (t::settings_backup_dest_label(locale)) }
                        input name="backup_dest" value=(field(&settings.backup_dest))
                            placeholder=(t::settings_backup_dest_placeholder(locale));
                        p class="muted" { (t::settings_backup_dest_hint(locale)) }
                    }
                    div class="field" {
                        label {
                            input type="checkbox" name="enabled" value="1" checked[settings.backup_enabled];
                            " " (t::settings_backup_enabled_hint(locale))
                        }
                    }
                    div class="field" {
                        label { (t::settings_backup_interval_label(locale)) }
                        input name="interval" value=(field(&settings.backup_interval))
                            placeholder=(t::settings_backup_interval_placeholder(locale));
                    }
                    div class="field" {
                        label {
                            input type="checkbox" name="snapshot" value="1"
                                checked[settings.backup_snapshot.unwrap_or(true)];
                            " " (t::settings_backup_snapshot_hint(locale))
                        }
                    }

                    h4 { (t::settings_backup_classes_heading(locale)) }
                    @for (name, on, hint) in [
                        ("class_git", selection.git, t::settings_backup_class_git_hint(locale)),
                        ("class_lfs", selection.lfs, t::settings_backup_class_lfs_hint(locale)),
                        ("class_releases", selection.releases, t::settings_backup_class_releases_hint(locale)),
                        ("class_avatars", selection.avatars, t::settings_backup_class_avatars_hint(locale)),
                        ("class_entities", selection.entities, t::settings_backup_class_entities_hint(locale)),
                        ("class_audit", selection.audit, t::settings_backup_class_audit_hint(locale)),
                        ("class_derived", selection.derived, t::settings_backup_class_derived_hint(locale)),
                    ] {
                        div class="field" {
                            label {
                                input type="checkbox" name=(name) value="1" checked[on];
                                " " (hint)
                            }
                        }
                    }

                    h4 { (t::settings_backup_retention_heading(locale)) }
                    div class="field" {
                        label { (t::settings_backup_keep_label(locale)) }
                        input name="keep" value=(num_field(settings.backup_keep))
                            placeholder=(t::settings_backup_keep_placeholder(locale));
                    }
                    div class="field" {
                        label { (t::settings_backup_audit_keep_label(locale)) }
                        input name="audit_keep_months" value=(num_field(settings.backup_audit_keep_months))
                            placeholder=(t::settings_backup_audit_keep_placeholder(locale));
                    }
                    div class="field" {
                        label { (t::settings_backup_delete_grace_label(locale)) }
                        input name="delete_grace_days" value=(num_field(settings.backup_delete_grace_days))
                            placeholder=(t::settings_backup_delete_grace_placeholder(locale));
                        p class="muted" { (t::settings_backup_delete_grace_hint(locale)) }
                    }
                    div class="field" {
                        label { (t::settings_backup_concurrency_label(locale)) }
                        input name="concurrency" value=(num_field(settings.backup_concurrency))
                            placeholder=(t::settings_backup_concurrency_placeholder(locale));
                    }
                    p { button type="submit" { (t::settings_save_button(locale)) } }
                }
                p class="muted" { (t::settings_backup_secrets_warning(locale)) }
                // Its own form, not a second button in the one above: "run
                // now" must never carry unsaved edits into a run.
                form method="post" action="/_settings/backup/run" {
                    (PreEscaped(csrf))
                    p { button type="submit" { (t::settings_backup_run_now_button(locale)) } }
                }
            }))
            (settings_section(&t::settings_backup_runs_heading(locale), None, html! {
                @if runs.is_empty() {
                    p class="muted" { (t::settings_backup_runs_empty(locale)) }
                } @else {
                    table {
                        thead {
                            tr {
                                th { (t::settings_backup_col_started(locale)) }
                                th { (t::settings_backup_col_outcome(locale)) }
                                th { (t::settings_backup_col_trigger(locale)) }
                                th { (t::settings_backup_col_duration(locale)) }
                                th { (t::settings_backup_col_copied(locale)) }
                                th { (t::settings_backup_col_skipped(locale)) }
                                th { (t::settings_backup_col_failed(locale)) }
                            }
                        }
                        tbody {
                            @for run in runs {
                                @let totals = run.totals();
                                tr {
                                    td { (relative_time_ago(run.started_at, now)) }
                                    td { (outcome_label(run.outcome)) }
                                    td { (trigger_label(run.trigger)) }
                                    td {
                                        @match run.duration_secs() {
                                            Some(secs) => (format!("{secs}s")),
                                            None => "-",
                                        }
                                    }
                                    td { (format!("{} ({})", totals.copied, bytes_human(totals.bytes_copied))) }
                                    td { (totals.skipped) }
                                    td { (run.failures_total) }
                                }
                            }
                        }
                    }
                    // Everything below is per-run detail that only exists
                    // when something needs an operator's attention.
                    @for run in runs {
                        @if let Some(error) = &run.error {
                            p class="flash flash-err" { (error) }
                        }
                        @if !run.unknown_keys.is_empty() {
                            p class="muted" {
                                (t::settings_backup_unknown_warning(locale))
                                @for key in &run.unknown_keys { " " code { (key) } }
                            }
                        }
                        @if !run.failures.is_empty() {
                            details {
                                summary { (t::settings_backup_failures_heading(locale)) " (" (run.failures_total) ")" }
                                ul { @for failure in &run.failures { li { code { (failure) } } } }
                            }
                        }
                    }
                }
            }))
        }
    }
}

pub(crate) async fn handler_settings_backup_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/backup").await {
        return Ok(resp);
    }
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let runs = report::list_recent(ctx.backend().as_ref(), &ctx.cipher, report::LIST_LIMIT).await?;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_backup_page(&settings, &runs, &csrf_field(&headers), None, locale);
    Ok(render_page(&ctx, &headers, "backup", content, locale).await)
}

pub(crate) async fn handler_settings_backup_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<BackupForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/backup").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (current, etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;

    // Everything that can be checked without touching the network is checked
    // here, so a typo is an error next to the field rather than a schedule
    // that quietly never works. `Dest::validate_against` re-reads
    // `gitquarry.toml` for exactly that reason.
    let settings = match build_backup_settings(&ctx, form, current.clone()) {
        Ok(settings) => settings,
        Err(message) => {
            let content = render_backup_page(&current, &[], &csrf_field(&headers), Some(Flash::Err(&message)), locale);
            if is_async(&headers) {
                return Ok(axum::response::Html(content.into_string()).into_response());
            }
            return Ok(render_page(&ctx, &headers, "backup", content, locale).await);
        }
    };

    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
    let flash =
        match storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher).await? {
            Cas::Ok => Flash::Ok("saved to the bucket."),
            Cas::Conflict => Flash::Err(&cas_conflict_msg),
        };
    let runs = report::list_recent(ctx.backend().as_ref(), &ctx.cipher, report::LIST_LIMIT).await?;
    let content = render_backup_page(&settings, &runs, &csrf_field(&headers), Some(flash), locale);
    if is_async(&headers) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    Ok(render_page(&ctx, &headers, "backup", content, locale).await)
}

/// Turns a submitted form into the settings to store, or into the one
/// message to show instead. `Err(String)` rather than `anyhow`: every one of
/// these is something an admin typed and can fix in the field it came from.
fn build_backup_settings(
    ctx: &AppContext,
    form: BackupForm,
    current: ServerSettings,
) -> Result<ServerSettings, String> {
    let dest = non_empty(form.backup_dest);
    if let Some(raw) = &dest {
        let target = Dest::parse(raw).map_err(|e| format!("{e:#}"))?;
        let config = gitquarry_core::config::Config::load_from(&ctx.config_path).map_err(|e| format!("{e:#}"))?;
        target
            .validate_against(&config, &ctx.bucket)
            .map_err(|e| format!("{e:#}"))?;
    }
    // Validated the same way the job itself would, so an interval that
    // wouldn't parse is refused here rather than silently falling back to
    // the default on every run.
    let interval = non_empty(form.interval);
    if let Some(raw) = &interval {
        gitquarry_core::mirror::gc::parse_age(raw).map_err(|e| format!("interval: {e:#}"))?;
    }

    Ok(ServerSettings {
        backup_dest: dest,
        backup_enabled: checked(&form.enabled),
        backup_interval: interval,
        backup_snapshot: Some(checked(&form.snapshot)),
        backup_include_git: Some(checked(&form.class_git)),
        backup_include_lfs: Some(checked(&form.class_lfs)),
        backup_include_releases: Some(checked(&form.class_releases)),
        backup_include_avatars: Some(checked(&form.class_avatars)),
        backup_include_entities: Some(checked(&form.class_entities)),
        backup_include_audit: Some(checked(&form.class_audit)),
        backup_include_derived: Some(checked(&form.class_derived)),
        backup_keep: parse_optional_u32(&form.keep, "snapshots to keep")?,
        backup_audit_keep_months: parse_optional_u32(&form.audit_keep_months, "audit months")?,
        backup_delete_grace_days: parse_optional_u32(&form.delete_grace_days, "delete grace days")?,
        backup_concurrency: parse_optional_u32(&form.concurrency, "concurrency")?,
        ..current
    })
}

/// Starts a run and returns immediately. A backup walks the whole bucket and
/// can take hours — tying it to the lifetime of an HTTP request would mean a
/// proxy timeout kills it half-done. The run records itself in the bucket
/// (`Outcome::Running` from the start), so reloading the page is how an
/// admin watches it.
pub(crate) async fn handler_settings_backup_run_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<BackupRunForm>,
) -> Result<Response, AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_settings/backup").await {
        return Ok(resp);
    }
    require_csrf(&headers, &form.csrf)?;
    let detached = Arc::clone(&ctx);
    tokio::spawn(async move {
        match schedule::run_once(&detached, Trigger::Manual).await {
            Ok(run) if run.outcome == Outcome::Failed => warn!(
                reason = run.error.as_deref().unwrap_or("no reason recorded"),
                "manual backup failed"
            ),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "manual backup couldn't be recorded"),
        }
    });

    let locale = effective_locale(&ctx, &headers).await;
    let (settings, _etag) = storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher).await?;
    let runs = report::list_recent(ctx.backend().as_ref(), &ctx.cipher, report::LIST_LIMIT).await?;
    let started = gitquarry_i18n::web::settings::settings_backup_run_started(locale);
    let content = render_backup_page(
        &settings,
        &runs,
        &csrf_field(&headers),
        Some(Flash::Ok(&started)),
        locale,
    );
    Ok(render_page(&ctx, &headers, "backup", content, locale).await)
}

// Only called by `settings_body`/`settings_full_page`, both test-only
// helpers — dead outside `test-util`, which is what `cargo check` flags.
#[cfg(any(test, feature = "test-util"))]
fn render_section(
    section: &str,
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    match section {
        "footer" => render_footer_page(settings, csrf, message, locale),
        "network" => render_network_page(settings, csrf, message, locale),
        "mirrors-cache" => render_mirrors_cache_page(settings, csrf, message, locale),
        "external-mirrors" => render_external_mirrors_page(settings, csrf, message, locale),
        "oidc" => render_oidc_page(settings, csrf, message, locale),
        // No run history in this test-only path: the recent-runs table is
        // rendered from a separate read the real handler does, and a caller
        // asking for "the backup section's markup" is asking about the form.
        "backup" => render_backup_page(settings, &[], csrf, message, locale),
        other => panic!("unknown settings section: {other}"),
    }
}

/// Just the section's own `#<section>-body` fragment — what an async POST
/// (see [`is_async`]) actually sends back, and what a stale bookmark on the
/// old single-page `/_settings` would have shown as the whole form.
#[cfg(any(test, feature = "test-util"))]
pub fn settings_body(
    section: &str,
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    render_section(section, settings, csrf, message, locale)
}

/// The backup section with a run history, which [`settings_body`] can't
/// reach: that one dispatches on a section name and only has the settings
/// object, while the recent-runs table comes from a separate read.
#[cfg(any(test, feature = "test-util"))]
pub fn settings_backup_body(
    settings: &ServerSettings,
    runs: &[BackupRun],
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    render_backup_page(settings, runs, csrf, message, locale)
}

/// The section's fragment wrapped in the sidebar shell [`settings_page`]
/// gives every real GET — what a plain (non-JS) load of `/_settings/<section>`
/// renders inside `layout`'s `<body>`.
#[cfg(any(test, feature = "test-util"))]
pub fn settings_full_page(
    section: &str,
    settings: &ServerSettings,
    csrf: &str,
    message: Option<Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    settings_page(
        section,
        render_section(section, settings, csrf, message, locale),
        locale,
    )
}
