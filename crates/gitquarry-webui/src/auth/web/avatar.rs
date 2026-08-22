//! `/_account/profile`: how the logged-in account presents itself — its
//! avatar and the email addresses it answers to — the "profile" section of
//! the account settings sidebar (`crate::webui::account_settings_page`).
//! Also owns the public image serve (`GET /_avatar/<username>`), unrelated
//! to the sidebar but keyed by username since it's a public URL embedded in
//! every `<img>` `crate::webui::identicon::account_avatar` renders anywhere
//! on the site, not just here.

use super::login;
use crate::webui::{
    AppError, State_, account_settings_page, csrf_field, effective_density, effective_locale, effective_theme, field,
    layout, require_csrf, require_login, settings_section,
};
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::auth::entities::User;
use gitquarry_core::auth::{access, domain};
use gitquarry_core::validate;
use maud::{Markup, PreEscaped, html};

/// Disk-fill guard on the scratch buffer the multipart body streams into
/// before it's uploaded — an avatar is a small, fixed-purpose image, not an
/// arbitrary release binary, so this is far tighter than
/// `repo::releases::MAX_ASSET_UPLOAD_BYTES`.
const MAX_AVATAR_UPLOAD_BYTES: usize = 2 * 1024 * 1024;

pub(crate) async fn handler_account_profile_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let user = match require_login(&ctx, &headers, "/_account/profile").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let topnav = super::login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_profile_section(&user, &csrf_field(&headers), ctx.oidc.is_none(), None, locale);
    let body = account_settings_page("profile", content, locale).into_string();
    Ok(Html(layout(
        "profile",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// The `/_account/profile` page: how this account presents itself — its
/// avatar and the addresses it answers to. Emails moved here from
/// `security`, which is now only about *signing in*: an address is an
/// identity, not a credential, and grouping it with passwords and 2FA made
/// that page a catch-all.
///
/// `email_self_service` is [`gitquarry_core::app::AppContext::oidc`] being
/// unset — see `domain::user::set_own_email` for why the primary address is
/// admin-managed wherever OIDC can resolve a login by it.
#[cfg(any(test, feature = "test-util"))]
pub fn render_profile_section(
    user: &User,
    csrf: &str,
    email_self_service: bool,
    message: Option<crate::webui::Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    render_profile_section_impl(user, csrf, email_self_service, message, locale)
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_profile_section(
    user: &User,
    csrf: &str,
    email_self_service: bool,
    message: Option<crate::webui::Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    render_profile_section_impl(user, csrf, email_self_service, message, locale)
}
fn render_profile_section_impl(
    user: &User,
    csrf: &str,
    email_self_service: bool,
    message: Option<crate::webui::Flash>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use crate::auth::i18n::security as t;
    html! {
        (PreEscaped(crate::webui::render_flash(message, locale)))

        (settings_section(&crate::auth::i18n::avatar::heading(locale), Some(&crate::auth::i18n::avatar::description(locale)), html! {
            p { (crate::webui::identicon::account_avatar_sized(&user.username, user.avatar_content_type.as_deref(), 48)) }
            form method="post" action="/_account/profile" enctype="multipart/form-data" {
                (PreEscaped(csrf))
                input type="file" name="avatar" accept="image/png,image/jpeg,image/webp" required;
                button type="submit" { (crate::auth::i18n::avatar::upload_button(locale)) }
            }
            @if user.avatar_content_type.is_some() {
                form method="post" action="/_account/profile/delete" data-confirm=(crate::auth::i18n::avatar::remove_avatar_confirm(locale)) {
                    (PreEscaped(csrf))
                    button type="submit" { (crate::auth::i18n::avatar::remove_button(locale)) }
                }
            }
        }))

        (settings_section(&t::primary_email_heading(locale), Some(&t::primary_email_purpose(locale)), render_primary_email_section(user, csrf, email_self_service, locale)))

        (settings_section(&t::secondary_emails_heading(locale), Some(&t::secondary_emails_purpose(locale)), html! {
            (render_secondary_emails_table(user, csrf, locale))
            form method="post" action=(EMAIL_ACTION_URL) data-async data-async-target="#secondary-emails-table" {
                (PreEscaped(csrf))
                input type="hidden" name="email_action" value="add_secondary_email";
                (field(&t::secondary_email_column_header(locale), None, html! {
                    input type="email" name="secondary_email" size="40" placeholder=(t::add_secondary_email_placeholder(locale));
                }))
                button type="submit" { (t::add_secondary_email_button(locale)) }
            }
        }))
    }
}

/// The primary-email form/display alone, wrapped in a stable `id` so
/// `set_email` can swap just this fragment — same reasoning
/// `render_secondary_emails_table` below already has for its own removal.
fn render_primary_email_section(
    user: &User,
    csrf: &str,
    email_self_service: bool,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use crate::auth::i18n::security as t;
    html! {
        div id="primary-email-section" {
            @if email_self_service {
                form method="post" action=(EMAIL_ACTION_URL) data-async data-async-target="#primary-email-section" data-confirm=(t::save_email_confirm(locale)) {
                    (PreEscaped(csrf))
                    input type="hidden" name="email_action" value="set_email";
                    (field(&t::primary_email_label(locale), Some(&t::primary_email_hint(locale)), html! {
                        input type="email" name="email" size="40" value=(user.email.clone().unwrap_or_default()) autocomplete="email";
                    }))
                    button type="submit" { (t::save_email_button(locale)) }
                }
            } @else {
                p {
                    @match &user.email {
                        Some(email) => code { (email) },
                        None => em { (t::no_primary_email(locale)) },
                    }
                }
                p class="muted" { (t::primary_email_admin_managed(locale)) }
            }
        }
    }
}

/// The secondary-emails table alone, wrapped in a stable `id` so a removal
/// can swap just this fragment.
fn render_secondary_emails_table(user: &User, csrf: &str, locale: gitquarry_i18n::Locale) -> Markup {
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
                            form method="post" action=(EMAIL_ACTION_URL) style="display:inline" data-async data-async-target="#secondary-emails-table" data-confirm=(t::remove_secondary_email_confirm(locale, email)) {
                                (PreEscaped(csrf))
                                input type="hidden" name="email_action" value="remove_secondary_email";
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

const EMAIL_ACTION_URL: &str = "/_account/profile/emails";

#[derive(serde::Deserialize)]
pub(crate) struct EmailForm {
    /// Wire name `email_action`, not `action` — same `[OverrideBuiltins]`
    /// reason [`super::keys::KeysForm`] documents for its own rename.
    #[serde(rename = "email_action")]
    action: String,
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    secondary_email: String,
}

/// `POST /_account/profile/emails` — its own route rather than the section's
/// own URL because that one takes a `multipart/form-data` avatar upload,
/// and a plain urlencoded form can't share an extractor with it.
pub(crate) async fn handler_emails_post(
    State(ctx): State_,
    headers: HeaderMap,
    axum::extract::Form(form): axum::extract::Form<EmailForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/profile").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let csrf = csrf_field(&headers);
    let self_service = ctx.oidc.is_none();

    // Async is different from the plain fallback below: the two targets an
    // async submit on this page can hit (`#primary-email-section`,
    // `#secondary-emails-table`) never contain a flash banner
    // (`render_profile_section`'s own shell has it, outside either), so a
    // page-level fragment swapped into one would corrupt it — same
    // reasoning `keys::handler_keys_post`'s own doc comment gives for its
    // own `fail!`. This goes through `AppError` instead: a real non-2xx
    // status, picked up by `extractFlashMessage`/`showErrorToast` the same
    // way any other failed `data-async` request already is.
    macro_rules! fail {
        ($msg:expr) => {{
            if headers.contains_key(crate::webui::ASYNC_HEADER) {
                return Err(AppError::bad_request(anyhow::anyhow!("{}", $msg)));
            }
            return Ok(axum::response::Html(layout(
                "profile",
                "",
                &account_settings_page(
                    "profile",
                    render_profile_section(&me, &csrf, self_service, Some(crate::webui::Flash::Err(&$msg)), locale),
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
        "set_email" => {
            match domain::user::set_own_email(
                &ctx,
                &me.username,
                crate::webui::non_empty(form.email.clone()).as_deref(),
                locale,
            )
            .await
            {
                Ok(cas) => cas,
                Err(e) => fail!(e.to_string()),
            }
        }
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
        other => fail!(gitquarry_i18n::shared::unknown_action(locale, other)),
    };
    if cas == gitquarry_core::storage::Cas::Conflict {
        fail!(gitquarry_i18n::shared::cas_conflict(locale));
    }

    let Some(me) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username)
        .await?
        .0
    else {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::account_no_longer_exists(locale)
        )));
    };
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        if form.action == "set_email" {
            return Ok(axum::response::Html(
                render_primary_email_section(&me, &csrf, self_service, locale).into_string(),
            )
            .into_response());
        }
        if matches!(form.action.as_str(), "add_secondary_email" | "remove_secondary_email") {
            return Ok(
                axum::response::Html(render_secondary_emails_table(&me, &csrf, locale).into_string()).into_response(),
            );
        }
    }
    let body = account_settings_page(
        "profile",
        render_profile_section(
            &me,
            &csrf,
            self_service,
            Some(crate::webui::Flash::Ok("saved.")),
            locale,
        ),
        locale,
    )
    .into_string();
    Ok(axum::response::Html(layout(
        "profile",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

pub(crate) async fn handler_avatar_upload_post(
    State(ctx): State_,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    // Before a byte of the body is read: an anonymous caller must not be
    // able to make this process buffer `MAX_AVATAR_UPLOAD_BYTES` (or stream
    // an unbounded number of ignored fields past it) just by POSTing here.
    // The session lives in the request headers, so this guard needs nothing
    // from the body — unlike `require_csrf` below, whose token is itself a
    // multipart field and so can only be checked once the body is parsed.
    let me = match require_login(&ctx, &headers, "/_account/profile").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let mut csrf = String::new();
    let mut uploaded: Option<Vec<u8>> = None;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::invalid_multipart(locale, e)
        ))
    })? {
        match field.name() {
            Some("csrf") => {
                csrf = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad_request(anyhow::anyhow!("{e}")))?;
            }
            Some("avatar") => {
                let mut buf = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(|e| {
                    AppError::bad_request(anyhow::anyhow!("{}", gitquarry_i18n::shared::reading_file(locale, e)))
                })? {
                    if buf.len() + chunk.len() > MAX_AVATAR_UPLOAD_BYTES {
                        return Err(AppError::bad_request(anyhow::anyhow!(
                            "{}",
                            gitquarry_i18n::shared::asset_too_large(locale, MAX_AVATAR_UPLOAD_BYTES as u64)
                        )));
                    }
                    buf.extend_from_slice(&chunk);
                }
                uploaded = Some(buf);
            }
            _ => {}
        }
    }

    require_csrf(&headers, &csrf)?;

    let buf = uploaded.ok_or_else(|| {
        AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::missing_file_to_upload(locale)
        ))
    })?;
    let content_type = validate::sniff_avatar_content_type(&buf).map_err(AppError::bad_request)?;

    let tmp = tempfile::Builder::new()
        .prefix(".gitquarry-avatar-")
        .tempfile()
        .map_err(|e| {
            AppError::bad_request(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::creating_temp_file(locale, e)
            ))
        })?
        .into_temp_path();
    tokio::fs::write(&tmp, &buf).await?;

    domain::user::set_avatar(&ctx, &me.username, content_type, &tmp, locale)
        .await
        .map_err(AppError::bad_request)?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "avatar.set",
        &me.username,
        None,
    )
    .await;
    Ok(Redirect::to("/_account/profile").into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct AvatarDeleteForm {
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_avatar_delete_post(
    State(ctx): State_,
    headers: HeaderMap,
    axum::extract::Form(form): axum::extract::Form<AvatarDeleteForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/profile").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;

    domain::user::remove_avatar(&ctx, &me.username, locale).await?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "avatar.remove",
        &me.username,
        None,
    )
    .await;
    Ok(Redirect::to("/_account/profile").into_response())
}

/// `GET /_avatar/<username>` — public, no login gate, same as the identicon
/// it replaces: this is what every `<img>` [`crate::webui::identicon::account_avatar`]
/// renders actually fetches.
pub(crate) async fn handler_avatar_get(State(ctx): State_, Path(username): Path<String>) -> Result<Response, AppError> {
    let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &username)
        .await?
        .0
    else {
        return Err(AppError::not_found(anyhow::anyhow!("no such user")));
    };
    let Some(content_type) = user.avatar_content_type else {
        return Err(AppError::not_found(anyhow::anyhow!("{username} has no avatar")));
    };

    let tmp_dir = tempfile::tempdir()?;
    let tmp_path = tmp_dir.path().join("avatar");
    ctx.avatar_remote().download_avatar(&username, &tmp_path).await?;
    let bytes = tokio::fs::read(&tmp_path).await?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "private, max-age=300".to_string()),
        ],
        bytes,
    )
        .into_response())
}
